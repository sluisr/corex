use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use crate::fs_tools::{format_size, is_sensitive_system_path, read_path_needs_confirmation, resolve_path};
use crate::types::{Tool, ToolContext, ToolOutput};

const OCR_URL: &str = "https://api.deepseek.com/v1/tools/ocr";
const OCR_MAX_BYTES: u64 = 100 * 1024 * 1024;
const DOCUMENT_EXTENSIONS: [&str; 13] =
    ["pdf", "doc", "docx", "ppt", "pptx", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

pub fn is_document_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DOCUMENT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Drops the `<｜box｜>` coordinates and `<｜ref｜>` tags page-by-page recognition leaves in the text.
pub fn clean_ocr_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<｜box｜>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("<｜/box｜>") {
            Some(end) => rest = &rest[start + end + "<｜/box｜>".len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.replace("<｜ref｜>", "").replace("<｜/ref｜>", "")
}

pub fn format_ocr_result(name: &str, result: &serde_json::Value) -> String {
    let pages = result["pages"].as_array().cloned().unwrap_or_default();
    let mut out = format!("{name}: {} page(s), text recognized by DeepSeek OCR.\n", pages.len());
    for page in &pages {
        let index = page["index"].as_u64().unwrap_or(0) + 1;
        out.push_str(&format!("\n--- page {index} ---\n"));
        out.push_str(clean_ocr_text(page["text"].as_str().unwrap_or_default()).trim());
        out.push('\n');
        if page["finish_reason"] == "length" {
            out.push_str("[this page's text is incomplete]\n");
        }
    }
    out
}

fn http_error(status: reqwest::StatusCode, body: &str) -> String {
    match status.as_u16() {
        402 => "DeepSeek balance is insufficient. Run /balance to check your credits.".to_string(),
        408 => "The upload to DeepSeek OCR was too slow and timed out.".to_string(),
        413 => "The file is larger than the 100 MiB OCR limit.".to_string(),
        429 => "Too many OCR recognitions are running on this account (15 at most). Retry shortly.".to_string(),
        503 => "DeepSeek OCR is busy. Retry later.".to_string(),
        _ => format!("DeepSeek OCR HTTP {status}: {body}"),
    }
}

pub struct ReadDocumentTool;

#[async_trait]
impl Tool for ReadDocumentTool {
    fn name(&self) -> &'static str {
        "read_document"
    }

    fn description(&self) -> &'static str {
        "Reads the text of a PDF, Word (doc/docx), PowerPoint (ppt/pptx) or scanned image with DeepSeek OCR and returns it page by page. \
         Use it for documents read_file cannot show. The file is sent to DeepSeek; billed per page (up to 1,000 pages, 100 MiB)."
    }

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        read_path_needs_confirmation(self.name(), args.get("file_path").and_then(|v| v.as_str()), context)
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path of the document (relative or absolute, supports ~/)."
                },
                "rich_layout": {
                    "type": "boolean",
                    "description": "Recognize every page as an image, keeping tables and layout. Default false: text is extracted directly where possible."
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let raw = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(ToolOutput::error("Missing 'file_path' argument.")),
        };
        let path = resolve_path(&context.workspace_dir, raw);
        if is_sensitive_system_path(&path) {
            return Ok(ToolOutput::error(format!("Access denied: '{raw}' is a sensitive credential file.")));
        }
        if !is_document_path(&path) {
            return Ok(ToolOutput::error(format!(
                "'{raw}' is not a supported document. read_document takes {}.",
                DOCUMENT_EXTENSIONS.join(", ")
            )));
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(ToolOutput::error(format!("File does not exist: {raw}")));
        };
        if !meta.is_file() {
            return Ok(ToolOutput::error(format!("'{raw}' is not a regular file.")));
        }
        if meta.len() > OCR_MAX_BYTES {
            return Ok(ToolOutput::error(format!(
                "'{raw}' is {}, over the 100 MiB OCR limit. Split it first.",
                format_size(meta.len())
            )));
        }
        let Some(api_key) = crate::web::resolve_api_key(context) else {
            return Ok(ToolOutput::error(
                "DeepSeek OCR is a DeepSeek tool and only runs with the DeepSeek provider active, so it is not used with this model. Extract the text locally instead (e.g. `pdftotext file.pdf -` via run_shell_command), or switch to a model that reads documents natively.",
            ));
        };

        let bytes = tokio::fs::read(&path).await?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("document").to_string();
        let purpose = if args.get("rich_layout").and_then(|v| v.as_bool()).unwrap_or(false) {
            "rich_layout_recognition"
        } else {
            "text_recognition"
        };
        let form = reqwest::multipart::Form::new()
            .text("purpose", purpose)
            .part("file", reqwest::multipart::Part::bytes(bytes).file_name(file_name.clone()));

        let res = match reqwest::Client::new()
            .post(OCR_URL)
            .bearer_auth(api_key)
            .multipart(form)
            .timeout(std::time::Duration::from_secs(190))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(format!("DeepSeek OCR request failed: {e}"))),
        };
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Ok(ToolOutput::error(http_error(status, body.trim())));
        }
        let result: serde_json::Value = match serde_json::from_str(body.trim()) {
            Ok(v) => v,
            Err(_) => return Ok(ToolOutput::error("DeepSeek OCR returned a response that is not JSON.")),
        };
        if let Some(message) = result["error"]["message"].as_str() {
            return Ok(ToolOutput::error(format!("DeepSeek OCR failed: {message}")));
        }
        let pages = result["page_count"].as_u64().unwrap_or(0);
        Ok(ToolOutput::success_with_summary(
            format_ocr_result(&file_name, &result),
            format!("Read {file_name} ({pages} pages)"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_and_ref_tags_are_dropped() {
        let raw = "<｜ref｜>Title<｜/ref｜><｜box｜>[[52, 40, 948, 96]]<｜/box｜>\nBody text";
        assert_eq!(clean_ocr_text(raw), "Title\nBody text");
        assert_eq!(clean_ocr_text("plain"), "plain");
    }

    #[test]
    fn pages_are_numbered_from_one_and_cut_pages_are_flagged() {
        let result = json!({
            "page_count": 2,
            "pages": [
                {"index": 0, "text": "first", "finish_reason": "stop"},
                {"index": 1, "text": "second", "finish_reason": "length"}
            ]
        });
        let out = format_ocr_result("a.pdf", &result);
        assert!(out.starts_with("a.pdf: 2 page(s)"), "{out}");
        assert!(out.contains("--- page 1 ---\nfirst\n"), "{out}");
        assert!(out.contains("--- page 2 ---\nsecond\n[this page's text is incomplete]"), "{out}");
    }

    #[test]
    fn only_document_formats_are_accepted() {
        assert!(is_document_path(Path::new("report.PDF")));
        assert!(is_document_path(Path::new("slides.pptx")));
        assert!(!is_document_path(Path::new("main.rs")));
        assert!(!is_document_path(Path::new("README")));
    }
}
