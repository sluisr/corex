use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use base64::Engine;
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
            "description": "What to do."
        },
        "url": { "type": "string", "description": "navigate: address to open." },
        "ref": { "type": "string", "description": "Element ref from the latest snapshot, e.g. e7 (click, double_click, right_click, hover, type, select, choose, upload; scroll: the area to move)." },
        "text": { "type": "string", "description": "type: text to enter. dialog: answer for a prompt(). wait: text to wait for on the page." },
        "gone": { "type": "boolean", "description": "wait: wait until the text disappears instead." },
        "query": { "type": "string", "description": "snapshot: only list elements whose line contains this text (searches the whole page)." },
        "tab": { "type": "integer", "description": "tabs: id of the tab to switch to (or to close with close: true)." },
        "close": { "type": "boolean", "description": "tabs: close the given tab instead of switching to it." },
        "accept": { "type": "boolean", "description": "dialog: true to accept (OK), false to dismiss (default true)." },
        "paths": { "type": "array", "items": { "type": "string" }, "description": "upload: absolute paths of local files to attach to the file input." },
        "submit": { "type": "boolean", "description": "type: press Enter afterwards." },
        "clear": { "type": "boolean", "description": "type: replace existing content (default true)." },
        "value": { "type": "string", "description": "select: option value or visible label of a native <select>. choose: visible text of the entry to pick." },
        "key": { "type": "string", "description": "press_key: Enter, Tab, Escape, Backspace, Delete, Space, arrows, PageUp, PageDown, Home, End, F1-F12, a single character, or a combination such as Control+A, Shift+Tab, Meta+S." },
        "direction": { "type": "string", "enum": ["up", "down"], "description": "scroll: direction." },
        "seconds": { "type": "number", "description": "wait: seconds to pause (max 15), or with text the longest to wait (default 10, max 30)." }
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
        "Controls a tab in the user's own browser through the Corex Browser Bridge extension: open pages, read them, click and type. \
         Workflow: navigate, then read the returned snapshot (an accessibility tree where interactive \
         elements carry [ref=eN] and states such as [invalid=true] or description=\"error text\"), then act on a ref. \
         Every action that changes the page returns a fresh snapshot; refs are only valid for the latest one. \
         On long pages pass query to snapshot to list only the matching elements from the whole page; \
         link addresses are shown only in query results, so use a query when you need a link's URL. \
         After saving or submitting, use wait with text (or with gone: true for a spinner such as 'Saving') instead of fixed pauses. \
         Links or buttons that open a new tab or popup switch to it automatically; use tabs to list, switch (tab) or close (tab + close) them. \
         scroll moves the area under the mouse, so pass ref to scroll a side panel, table or list instead of the page. For a custom dropdown or menu (not a native <select>) use choose with the visible text of the entry: \
         it opens the control and picks the entry in one step. Use hover for menus that open on mouse-over, \
         upload to attach local files to a file input, and dialog to answer an alert/confirm/prompt \
         (an open dialog is reported in the console output). The snapshot is text only: use screenshot to actually look at the page \
         (images, charts, thumbnails, visual layout, a CAPTCHA, or to check that an action did what you expected), \
         especially after landing on a new page and whenever the snapshot is ambiguous. \
         Skip it when you are only reading text. Needs a vision model. \
         The browser is the user's own, so their logins are already there. \
         On a CAPTCHA, login wall or 2FA prompt, stop and ask the user to complete it in the browser tab."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": action_properties(), "required": ["action"] })
    }

    fn needs_confirmation(&self, args: &serde_json::Value, _context: &ToolContext) -> bool {
        changes_the_page(args)
    }

    async fn execute(&self, args: serde_json::Value, _context: &ToolContext) -> Result<ToolOutput> {
        let Some(action) = arg(&args, "action") else {
            return Ok(ToolOutput::error("Missing 'action' argument."));
        };

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
            match Browser::start().await {
                Ok(browser) => *guard = Some(browser),
                Err(e) => {
                    return Ok(ToolOutput::error(format!("{e}")));
                }
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
            let paths: Vec<String> = args
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|list| list.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
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
            let size = base64::engine::general_purpose::STANDARD
                .decode(&shot.base64)
                .map(|b| b.len())
                .unwrap_or(0);
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
