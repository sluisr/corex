use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use corex_browser::{Browser, Gesture};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::types::{Tool, ToolContext, ToolOutput};

static BROWSER: Mutex<Option<Browser>> = Mutex::const_new(None);

const READ_ONLY_ACTIONS: &[&str] = &["snapshot", "screenshot", "console", "text", "scroll", "wait", "hover", "tabs"];

pub struct BrowserTool;

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

fn flag(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn changes_the_page(args: &Value) -> bool {
    let action = arg(args, "action").unwrap_or("");
    let closes_tab = action == "tabs" && flag(args, "close", false);
    closes_tab || (!READ_ONLY_ACTIONS.contains(&action) && action != "close")
}

fn action_properties() -> Value {
    json!({
        "action": {
            "type": "string",
            "enum": ["navigate", "snapshot", "click", "double_click", "right_click", "hover", "type",
                     "select", "choose", "upload", "dialog", "press_key", "scroll", "back", "text",
                     "screenshot", "console", "wait", "tabs", "close"],
            "description": "Browser action."
        },
        "url": { "type": "string", "description": "navigate: target URL." },
        "ref": { "type": "string", "description": "Element ref from latest snapshot, e.g. e7 (click/double_click/right_click/hover/type/select/choose/upload, or area for scroll)." },
        "text": { "type": "string", "description": "type: text to enter; dialog: prompt() answer; wait: page text to await." },
        "gone": { "type": "boolean", "description": "wait: wait until text disappears." },
        "query": { "type": "string", "description": "snapshot: filter elements matching text across full page (also shows link URLs)." },
        "tab": { "type": "integer", "description": "tabs: tab id to switch to (or close with close:true)." },
        "close": { "type": "boolean", "description": "tabs: close tab instead of switching." },
        "accept": { "type": "boolean", "description": "dialog: accept (true, default) or dismiss (false)." },
        "paths": { "type": "array", "items": { "type": "string" }, "description": "upload: absolute local file paths." },
        "submit": { "type": "boolean", "description": "type: press Enter after typing." },
        "clear": { "type": "boolean", "description": "type: replace existing value (default true)." },
        "value": { "type": "string", "description": "select: <select> value/label; choose: visible menu option text." },
        "key": { "type": "string", "description": "press_key: key or chord (Enter, Escape, Tab, PageDown, Control+A, Shift+Tab, etc.)." },
        "direction": { "type": "string", "enum": ["up", "down"], "description": "scroll direction." },
        "seconds": { "type": "number", "description": "wait: pause seconds (max 15), or timeout with text (default 10, max 30)." }
    })
}

async fn with_snapshot(browser: &mut Browser, header: String) -> Result<ToolOutput> {
    let header = match browser.sync_tabs().await {
        Ok(Some(note)) => format!("{header}\n{note}"),
        Ok(None) => header,
        Err(e) => return Ok(ToolOutput::error(format!("{header}, but then {e}. Run the next action to open a new tab."))),
    };
    if let Some(dialog) = browser.open_dialog() {
        return Ok(ToolOutput::success(format!(
            "{header}\nA JavaScript dialog is open ({dialog}). The page is blocked until you answer it \
             with the dialog action (accept true or false)."
        )));
    }
    let snapshot = browser.snapshot().await?;
    Ok(ToolOutput::success(format!("{header}\n{snapshot}")))
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Drives a tab in the user's own logged-in browser via Corex Browser Bridge. \
         Workflow: navigate -> read accessibility snapshot ([ref=eN], [invalid=true], error descriptions) -> act on ref. \
         Mutating actions return a fresh snapshot (old refs expire). Pass query to snapshot on long pages or to reveal link URLs. \
         Use wait with text (or gone:true for spinners) instead of fixed delays. New tabs/popups focus automatically; manage with tabs. \
         Pass ref to scroll inner panels/lists; use choose for custom non-<select> dropdowns, hover for hover-menus, upload for file inputs, dialog for JS alerts/prompts, text for long copy. \
         Use screenshot on vision models to inspect images/charts/layout or verify actions. Stop and ask the user on CAPTCHA, login wall, or 2FA."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": action_properties(), "required": ["action"] })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, _context: &ToolContext) -> bool {
        changes_the_page(args)
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let Some(action) = arg(&args, "action") else {
            return Ok(ToolOutput::error("Missing 'action' argument."));
        };
        if let Some(reason) = refusal(action, &args, context) {
            return Ok(ToolOutput::error(reason));
        }

        let mut guard = BROWSER.lock().await;
        if action == "close" {
            *guard = None;
            return Ok(ToolOutput::success("Browser closed."));
        }
        if action == "wait" && arg(&args, "text").is_none() {
            let seconds = args.get("seconds").and_then(|v| v.as_f64()).unwrap_or(1.0).clamp(0.0, 15.0);
            tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
            return Ok(ToolOutput::success(format!("Waited {seconds}s.")));
        }

        let mut note = None;
        if let Some(browser) = guard.as_mut().filter(|browser| browser.is_alive()) {
            match browser.sync_tabs().await {
                Ok(changed) => note = changed,
                Err(_) => {
                    *guard = None;
                    note = Some("The previous Corex tab was closed, so a new one was opened.".to_string());
                }
            }
        }
        if !guard.as_ref().is_some_and(Browser::is_alive) {
            let was_open = guard.is_some() || note.is_some();
            match Browser::start().await {
                Ok(browser) => *guard = Some(browser),
                Err(e) => {
                    return Ok(ToolOutput::error(format!("{e}")));
                }
            }
            if let Some(reference) = arg(&args, "ref").filter(|_| action != "scroll") {
                let why = if was_open { "The connection to the browser was lost" } else { "No browser was open in this session" };
                return Ok(ToolOutput::error(format!(
                    "{why}, so Corex opened a new blank tab. Refs from earlier snapshots, such as '{reference}', \
                     no longer exist: navigate to the page again and act on the refs of its new snapshot."
                )));
            }
        }
        let browser = guard.as_mut().expect("browser started above");

        let outcome = run(browser, action, &args)
            .await
            .unwrap_or_else(|e| ToolOutput::error(format!("browser {action} failed: {e}")));
        Ok(match note {
            Some(note) => ToolOutput { output: format!("{note}\n{}", outcome.output), ..outcome },
            None => outcome,
        })
    }
}

fn upload_paths(args: &Value) -> Vec<String> {
    args.get("paths")
        .and_then(|v| v.as_array())
        .map(|list| list.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn refusal(action: &str, args: &Value, context: &ToolContext) -> Option<String> {
    let is_secret = |path: &str| {
        crate::fs_tools::is_sensitive_system_path(&crate::fs_tools::resolve_path(&context.workspace_dir, path))
    };
    match action {
        "upload" => upload_paths(args)
            .into_iter()
            .find(|path| is_secret(path))
            .map(|path| format!("Access denied: uploading the credential file '{path}' is forbidden.")),
        "navigate" => {
            let url = arg(args, "url")?;
            let scheme = url.split_once("://").map(|(scheme, _)| scheme.to_ascii_lowercase());
            match scheme.as_deref() {
                None | Some("http" | "https") => None,
                Some("file") => reqwest::Url::parse(url)
                    .ok()
                    .and_then(|parsed| parsed.to_file_path().ok())
                    .is_none_or(|path| is_secret(&path.to_string_lossy()))
                    .then(|| format!("Access denied: '{url}' cannot be opened in the browser.")),
                Some(_) => Some(format!("'{url}' cannot be opened: only http, https and file addresses are.")),
            }
        }
        _ => None,
    }
}

fn target<'a>(args: &'a Value, action: &str) -> Result<&'a str> {
    arg(args, "ref").ok_or_else(|| anyhow!("'{action}' needs the 'ref' argument"))
}

fn required<'a>(args: &'a Value, action: &str, name: &str) -> Result<&'a str> {
    arg(args, name).ok_or_else(|| anyhow!("'{action}' needs the '{name}' argument"))
}

async fn perform(browser: &mut Browser, action: &str, args: &Value) -> Result<String> {
    match action {
        "navigate" => {
            let url = required(args, action, "url")?;
            let url = if url.contains("://") || url.starts_with("about:") { url.to_string() } else { format!("https://{url}") };
            browser.navigate(&url).await?;
            Ok(format!("Opened {url}"))
        }
        "click" => {
            let key = target(args, action)?;
            browser.click(key).await?;
            Ok(format!("Clicked {key}"))
        }
        "double_click" | "right_click" | "hover" => {
            let key = target(args, action)?;
            let (gesture, done) = match action {
                "double_click" => (Gesture::Double, "Double-clicked"),
                "right_click" => (Gesture::Right, "Right-clicked"),
                _ => (Gesture::Hover, "Hovered"),
            };
            browser.pointer(key, gesture).await?;
            Ok(format!("{done} {key}"))
        }
        "choose" => {
            let value = required(args, action, "value")?;
            let key = target(args, action)?;
            browser.choose(key, value).await?;
            Ok(format!("Chose '{value}' from {key}"))
        }
        "upload" => {
            let paths = upload_paths(args);
            if paths.is_empty() {
                return Err(anyhow!("'upload' needs the 'paths' argument"));
            }
            let key = target(args, action)?;
            browser.upload(key, &paths).await?;
            Ok(format!("Attached {} file(s) to {key}", paths.len()))
        }
        "dialog" => {
            let accept = flag(args, "accept", true);
            browser.dialog(accept, arg(args, "text")).await?;
            Ok(format!("Dialog {}", if accept { "accepted" } else { "dismissed" }))
        }
        "type" => {
            let text = args.get("text").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("'type' needs the 'text' argument"))?;
            let key = target(args, action)?;
            browser.type_text(key, text, flag(args, "clear", true), flag(args, "submit", false)).await?;
            Ok(format!("Typed into {key}"))
        }
        "select" => {
            let value = required(args, action, "value")?;
            let key = target(args, action)?;
            browser.select_option(key, value).await?;
            Ok(format!("Selected '{value}' in {key}"))
        }
        "press_key" => {
            let key = required(args, action, "key")?;
            browser.press_key(key).await?;
            Ok(format!("Pressed {key}"))
        }
        "scroll" => {
            browser.scroll(arg(args, "direction").unwrap_or("down"), arg(args, "ref")).await?;
            Ok("Scrolled".to_string())
        }
        "back" => {
            browser.back().await?;
            Ok("Went back".to_string())
        }
        "wait" => {
            let Some(text) = arg(args, "text") else {
                let seconds = args.get("seconds").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 15.0);
                tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                return Ok(format!("Waited {seconds}s"));
            };
            let gone = flag(args, "gone", false);
            let seconds = args.get("seconds").and_then(Value::as_f64).unwrap_or(10.0).clamp(0.5, 30.0);
            let what = if gone { "disappeared" } else { "appeared" };
            if browser.wait_for_text(text, gone, Duration::from_secs_f64(seconds)).await? {
                Ok(format!("'{text}' {what}"))
            } else {
                Err(anyhow!("'{text}' had not {what} after {seconds}s"))
            }
        }
        "tabs" => {
            let tab = args.get("tab").and_then(Value::as_i64).ok_or_else(|| anyhow!("'tabs' needs the 'tab' argument"))?;
            if flag(args, "close", false) {
                browser.close_tab(tab).await?;
                return Ok(format!("Closed tab {tab}"));
            }
            browser.switch_tab(tab).await?;
            Ok(format!("Switched to tab {tab}"))
        }
        other => Err(anyhow!("unknown browser action '{other}'")),
    }
}

async fn run(browser: &mut Browser, action: &str, args: &Value) -> Result<ToolOutput> {
    if let Some(dialog) = browser.open_dialog().filter(|_| !matches!(action, "dialog" | "console")) {
        return Ok(ToolOutput::error(format!(
            "A JavaScript dialog is open ({dialog}) and blocks the page. Answer it with the dialog action first."
        )));
    }

    match action {
        "snapshot" => Ok(ToolOutput::success(browser.snapshot_matching(arg(args, "query")).await?)),
        "text" => {
            let mut text = browser.page_text().await?;
            if text.len() > 30_000 {
                let mut end = 30_000;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                text.push_str("\n… truncated");
            }
            Ok(ToolOutput::success(format!("{}\n\n{}", browser.page_info().await, text)))
        }
        "screenshot" => {
            let shot = browser.screenshot().await?;
            let size = shot.base64.trim_end_matches('=').len() * 3 / 4;
            corex_core::push_tool_image(
                "browser screenshot",
                format!("data:{};base64,{}", shot.media_type, shot.base64),
            );
            Ok(ToolOutput::success_with_summary(
                format!("Screenshot captured ({} KB). The image is attached in the next message.", size / 1024),
                "Browser screenshot",
            ))
        }
        "tabs" if args.get("tab").is_none() => {
            let lines: Vec<String> = browser
                .list_tabs()
                .await
                .into_iter()
                .map(|t| format!("- tab {}{}: {} ({})", t.id, if t.active { " [active]" } else { "" }, t.title, t.url))
                .collect();
            Ok(ToolOutput::success(lines.join("\n")))
        }
        "console" => {
            let lines = browser.take_console();
            if lines.is_empty() {
                Ok(ToolOutput::success("No console output since the last check."))
            } else {
                Ok(ToolOutput::success(lines.join("\n")))
            }
        }
        _ => match perform(browser, action, args).await {
            Ok(header) => with_snapshot(browser, header).await,
            Err(e) if action == "wait" => {
                let outcome = with_snapshot(browser, e.to_string()).await?;
                Ok(ToolOutput::error(outcome.output))
            }
            Err(e) => Err(e),
        },
    }
}
