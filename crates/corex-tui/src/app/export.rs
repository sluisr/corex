use std::collections::HashMap;
use std::path::{Path, PathBuf};

use corex_core::session::Session;
use corex_core::types::Message;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ExportFormat {
    Markdown,
    Text,
    Html,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Text => "txt",
            ExportFormat::Html => "html",
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct ExportRequest {
    pub format: ExportFormat,
    pub full: bool,
    pub path: Option<String>,
}

impl ExportRequest {
    pub(crate) fn parse(args: &[&str]) -> Result<ExportRequest, String> {
        let mut request = ExportRequest { format: ExportFormat::Markdown, full: false, path: None };
        for arg in args {
            match arg.to_ascii_lowercase().as_str() {
                "md" | "markdown" => request.format = ExportFormat::Markdown,
                "txt" | "text" => request.format = ExportFormat::Text,
                "html" | "pdf" => request.format = ExportFormat::Html,
                "full" => request.full = true,
                _ if request.path.is_none() => request.path = Some(arg.to_string()),
                _ => return Err(format!("Unexpected '{}'. Usage: /export [md|txt|html] [full] [path]", arg)),
            }
        }
        Ok(request)
    }

    pub(crate) fn target(&self, workspace: &Path, home: &Path, root: &Path, stamp: &str, title: &str) -> PathBuf {
        let file_name = format!("{}_{}.{}", stamp, slug(title), self.format.extension());
        match self.path.as_deref() {
            None => root.join(project_name(workspace, home)).join(file_name),
            Some(path) => {
                let path = corex_fleet::expand_tilde(Path::new(path));
                let path = if path.is_absolute() { path } else { workspace.join(path) };
                if path.is_dir() {
                    path.join(file_name)
                } else {
                    path
                }
            }
        }
    }
}

pub(crate) fn default_exports_root(home: &Path) -> PathBuf {
    home.join(".corex").join("exports")
}

/// The folder `exports_dir` names, or why it cannot be used.
pub(crate) fn exports_root(setting: Option<&str>, home: &Path) -> Result<PathBuf, String> {
    let Some(raw) = setting.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(default_exports_root(home));
    };
    let path = corex_fleet::expand_tilde(Path::new(raw));
    if !path.is_absolute() {
        return Err(format!("exports_dir \"{raw}\" in settings.json must be a full path such as ~/Documents/corex"));
    }
    if path.exists() && !path.is_dir() {
        return Err(format!("exports_dir \"{raw}\" in settings.json is a file, not a folder"));
    }
    Ok(path)
}

fn project_name(workspace: &Path, home: &Path) -> String {
    if workspace == home {
        return "home".to_string();
    }
    workspace
        .file_name()
        .map(|name| slug(&name.to_string_lossy()))
        .filter(|name| name != "chat")
        .unwrap_or_else(|| "home".to_string())
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "chat".to_string() } else { out.to_string() }
}

fn unused_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}-{n}.{ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused name exists")
}

enum Entry {
    User(String),
    Reply(String),
    Tool { name: String, detail: String, output: String },
}

const TOOL_LINE_MAX: usize = 120;

fn shorten(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

fn call_detail(arguments: &str) -> String {
    let Ok(serde_json::Value::Object(args)) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return String::new();
    };
    ["command", "path", "file_path", "url", "query", "pattern", "prompt"]
        .iter()
        .find_map(|key| args.get(*key).and_then(|v| v.as_str()))
        .or_else(|| args.values().find_map(|v| v.as_str()))
        .map(|v| shorten(v, 80))
        .unwrap_or_default()
}

fn entries(messages: &[Message]) -> Vec<Entry> {
    let mut calls: HashMap<String, (String, String)> = HashMap::new();
    let mut out = Vec::new();
    for msg in messages {
        match msg.role.as_str() {
            "user" if !corex_core::is_tool_image_message(msg) => {
                if let Some(text) = msg.display_text().filter(|t| !t.trim().is_empty()) {
                    out.push(Entry::User(text.into_owned()));
                }
            }
            "assistant" => {
                if let Some(text) = msg.text_content().filter(|t| !t.trim().is_empty()) {
                    out.push(Entry::Reply(text.to_string()));
                }
                for call in msg.tool_calls.iter().flatten() {
                    calls.insert(call.id.clone(), (call.function.name.clone(), call_detail(&call.function.arguments)));
                }
            }
            "tool" => {
                let (name, detail) = msg
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| calls.get(id).cloned())
                    .unwrap_or_else(|| (msg.name.clone().unwrap_or_else(|| "tool".to_string()), String::new()));
                out.push(Entry::Tool { name, detail, output: msg.text_content().unwrap_or("").to_string() });
            }
            _ => {}
        }
    }
    out
}

fn tool_summary(name: &str, detail: &str, output: &str) -> String {
    let lines = output.lines().count();
    let mut text = name.to_string();
    if !detail.is_empty() {
        text.push_str(&format!(" `{}`", detail.replace('`', "'")));
    }
    let first = shorten(output, TOOL_LINE_MAX);
    if !first.is_empty() {
        text.push_str(&format!(" → {}", first));
    }
    if lines > 1 {
        text.push_str(&format!(" ({} lines)", lines));
    }
    text
}

fn fence_for(text: &str) -> String {
    let longest = text
        .lines()
        .map(|l| l.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

pub(crate) struct ExportMeta<'a> {
    pub user: &'a str,
    pub model: &'a str,
    pub date: &'a str,
}

pub(crate) fn to_markdown(session: &Session, meta: &ExportMeta, full: bool) -> String {
    let mut out = format!("# {}\n\n_Exported from Corex · {} · {}_\n", title(session), meta.date, meta.model);
    let mut in_tools = false;
    for entry in entries(session.display_messages()) {
        match entry {
            Entry::User(text) => {
                out.push_str(&format!("\n## {}\n\n{}\n", meta.user, text.trim_end()));
                in_tools = false;
            }
            Entry::Reply(text) => {
                out.push_str(&format!("\n## Corex\n\n{}\n", text.trim_end()));
                in_tools = false;
            }
            Entry::Tool { name, detail, output } if full => {
                let fence = fence_for(&output);
                let label = if detail.is_empty() { name } else { format!("{} `{}`", name, detail.replace('`', "'")) };
                out.push_str(&format!("\n**Tool:** {}\n\n{fence}\n{}\n{fence}\n", label, output.trim_end()));
            }
            Entry::Tool { name, detail, output } => {
                if !in_tools {
                    out.push('\n');
                    in_tools = true;
                }
                out.push_str(&format!("- {}\n", tool_summary(&name, &detail, &output)));
            }
        }
    }
    out
}

pub(crate) fn to_text(session: &Session, meta: &ExportMeta, full: bool) -> String {
    let mut out = format!("{}\nExported from Corex · {} · {}\n", title(session), meta.date, meta.model);
    for entry in entries(session.display_messages()) {
        match entry {
            Entry::User(text) => out.push_str(&format!("\n{}:\n{}\n", meta.user, text.trim_end())),
            Entry::Reply(text) => out.push_str(&format!("\nCorex:\n{}\n", text.trim_end())),
            Entry::Tool { name, detail, output } if full => {
                out.push_str(&format!("\n[{} {}]\n{}\n", name, detail, output.trim_end()));
            }
            Entry::Tool { name, detail, output } => {
                out.push_str(&format!("  [tool] {}\n", tool_summary(&name, &detail, &output).replace('`', "")));
            }
        }
    }
    out
}

const HTML_STYLE: &str = "body{font:16px/1.6 system-ui,-apple-system,'Segoe UI',sans-serif;max-width:780px;margin:40px auto;padding:0 16px;color:#1f2328;background:#fff}\
h1{font-size:1.6em;margin-bottom:0}h2{font-size:1.05em;margin:2em 0 .4em;padding-bottom:.2em;border-bottom:1px solid #d0d7de}\
em{color:#59636e}pre{background:#f6f8fa;border:1px solid #d0d7de;border-radius:6px;padding:12px;overflow-x:auto;white-space:pre-wrap;word-break:break-word}\
code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.9em}:not(pre)>code{background:#eff1f3;padding:.1em .3em;border-radius:4px}\
table{border-collapse:collapse}td,th{border:1px solid #d0d7de;padding:4px 10px}li{margin:.15em 0}\
@media print{body{margin:0;max-width:none}pre{break-inside:avoid}}";

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(crate) fn to_html(session: &Session, meta: &ExportMeta, full: bool) -> String {
    use pulldown_cmark::{html, Event, Options, Parser};

    let markdown = to_markdown(session, meta, full);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    // Chat text is untrusted, so raw HTML in it is shown, never run.
    let events = Parser::new_ext(&markdown, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        other => other,
    });
    let mut body = String::new();
    html::push_html(&mut body, events);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>{}</style>\n</head>\n<body>\n{}</body>\n</html>\n",
        escape_html(&title(session)),
        HTML_STYLE,
        body
    )
}

fn title(session: &Session) -> String {
    let title = session.title.trim();
    if !title.is_empty() && title != "New Session" {
        return title.to_string();
    }
    session
        .display_messages()
        .iter()
        .filter(|m| m.role == "user" && !corex_core::is_tool_image_message(m))
        .find_map(|m| m.display_text().and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(50).collect())))
        .unwrap_or_else(|| "Corex chat".to_string())
}

pub(crate) fn render(session: &Session, meta: &ExportMeta, request: &ExportRequest) -> String {
    match request.format {
        ExportFormat::Markdown => to_markdown(session, meta, request.full),
        ExportFormat::Text => to_text(session, meta, request.full),
        ExportFormat::Html => to_html(session, meta, request.full),
    }
}

impl super::App {
    pub(crate) fn export_chat(&self, request: &ExportRequest) -> String {
        let now = chrono::Local::now();
        let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) else {
            return "Could not find your home folder to save the export.".to_string();
        };
        let stamp = now.format("%Y-%m-%d_%H-%M").to_string();
        let setting = corex_core::config::Config::load_with_workspace(Some(&self.workspace_dir)).exports_dir;
        let (root, warning) = match exports_root(setting.as_deref(), &home) {
            Ok(root) => (root, String::new()),
            Err(why) => (default_exports_root(&home), format!(" ({why}, so the default folder was used.)")),
        };
        let mut target = request.target(&self.workspace_dir, &home, &root, &stamp, &title(&self.session));
        if request.path.is_none() {
            target = unused_path(target);
        }
        if let Some(dir) = target.parent() {
            if let Err(err) = std::fs::create_dir_all(dir) {
                return format!("Could not create {}: {}", dir.display(), err);
            }
        }
        let user = super::render::user_label();
        let model = self.llm_client.get_config().model;
        let date = now.format("%Y-%m-%d %H:%M").to_string();
        let meta = ExportMeta { user: &user, model: &model, date: &date };
        let content = render(&self.session, &meta, request);
        if let Err(err) = std::fs::write(&target, content) {
            return format!("Could not write {}: {}", target.display(), err);
        }
        let hint = if request.format == ExportFormat::Html {
            " Open it in a browser and print it (Ctrl+P) to save a PDF."
        } else {
            ""
        };
        format!("Chat exported to {}.{}{}", target.display(), hint, warning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corex_core::types::ToolCall;

    fn meta() -> ExportMeta<'static> {
        ExportMeta { user: "sluisr", model: "deepseek-flash", date: "2026-11-09 22:50" }
    }

    fn chat() -> Session {
        let mut session = Session::new_with_params("deepseek-flash", 0.7, "medium", None);
        session.add_message(Message::user("how full is the disk?"));
        session.title = "Check the disk".to_string();
        let mut call = Message { role: "assistant".to_string(), ..Message::user("") };
        call.tool_calls = Some(vec![ToolCall::new("c1", "run_shell_command", r#"{"command":"df -h /"}"#)]);
        session.add_message(call);
        session.add_message(Message::tool_response("c1", "Filesystem Size Used\n/dev/sda1 100G 42G\n<script>x</script>"));
        session.add_message(Message { role: "assistant".to_string(), ..Message::user("It is 42% full:\n```bash\ndf -h /\n```") });
        session.add_message(Message::system("Copied to clipboard"));
        session
    }

    #[test]
    fn markdown_keeps_the_conversation_and_sums_up_tools() {
        let md = to_markdown(&chat(), &meta(), false);
        assert!(md.starts_with("# Check the disk\n\n_Exported from Corex · 2026-11-09 22:50 · deepseek-flash_"), "{md}");
        assert!(md.contains("## sluisr\n\nhow full is the disk?"), "{md}");
        assert!(md.contains("- run_shell_command `df -h /` → Filesystem Size Used (3 lines)"), "{md}");
        assert!(md.contains("## Corex\n\nIt is 42% full:\n```bash\ndf -h /\n```"), "{md}");
        assert!(!md.contains("/dev/sda1") && !md.contains("Copied to clipboard"), "{md}");
    }

    #[test]
    fn full_exports_include_tool_output() {
        let md = to_markdown(&chat(), &meta(), true);
        assert!(md.contains("**Tool:** run_shell_command `df -h /`\n\n```\nFilesystem Size Used\n/dev/sda1 100G 42G"), "{md}");
    }

    #[test]
    fn text_is_plain() {
        let txt = to_text(&chat(), &meta(), false);
        assert!(txt.contains("\nsluisr:\nhow full is the disk?\n"), "{txt}");
        assert!(txt.contains("  [tool] run_shell_command df -h / → Filesystem Size Used (3 lines)"), "{txt}");
        assert!(txt.contains("\nCorex:\nIt is 42% full:"), "{txt}");
    }

    #[test]
    fn html_never_runs_chat_markup() {
        let html = to_html(&chat(), &meta(), true);
        assert!(html.starts_with("<!doctype html>") && html.contains("<title>Check the disk</title>"), "{html}");
        assert!(html.contains("<pre><code class=\"language-bash\">df -h /"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }

    #[test]
    fn export_arguments() {
        assert_eq!(
            ExportRequest::parse(&["html", "full", "~/Desktop"]).unwrap(),
            ExportRequest { format: ExportFormat::Html, full: true, path: Some("~/Desktop".to_string()) }
        );
        assert_eq!(ExportRequest::parse(&["pdf"]).unwrap().format, ExportFormat::Html);
        assert!(ExportRequest::parse(&["a.md", "b.md"]).is_err());
        let request = ExportRequest::parse(&[]).unwrap();
        let home = Path::new("/home/me");
        let root = default_exports_root(home);
        assert_eq!(
            request.target(Path::new("/home/me/proyects/cx"), home, &root, "2026-11-09_22-50", "Revisa el día ¿qué pasó?"),
            PathBuf::from("/home/me/.corex/exports/cx/2026-11-09_22-50_revisa-el-dia-que-paso.md")
        );
        assert_eq!(
            request.target(home, home, Path::new("/data/chats"), "2026-11-09_22-50", "!!!"),
            PathBuf::from("/data/chats/home/2026-11-09_22-50_chat.md")
        );
        let named = ExportRequest::parse(&["txt", "notes/today.txt"]).unwrap();
        assert_eq!(named.target(Path::new("/w"), home, &root, "s", "t"), PathBuf::from("/w/notes/today.txt"));
    }

    #[test]
    fn the_exports_dir_setting_is_checked() {
        let home = std::env::temp_dir().join(format!("corex-exports-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let file = home.join("a-file");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(exports_root(None, &home), Ok(home.join(".corex/exports")));
        assert_eq!(exports_root(Some("  "), &home), Ok(home.join(".corex/exports")));
        assert_eq!(exports_root(Some("/srv/chats"), &home), Ok(PathBuf::from("/srv/chats")));
        assert!(exports_root(Some("chats"), &home).unwrap_err().contains("must be a full path"));
        assert!(exports_root(Some(file.to_str().unwrap()), &home).unwrap_err().contains("is a file, not a folder"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_fence_inside_tool_output_does_not_end_the_block() {
        assert_eq!(fence_for("```\ncode\n```"), "````");
        assert_eq!(fence_for("plain"), "```");
    }
}
