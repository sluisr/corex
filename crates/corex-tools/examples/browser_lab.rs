use corex_tools::{ToolContext, ToolRegistry};
use serde_json::{json, Value};

fn ref_of(snapshot: &str, needle: &str) -> String {
    snapshot
        .lines()
        .find(|l| l.trim_start().starts_with("- ") && l.contains(needle))
        .and_then(|l| l.split("[ref=").nth(1))
        .and_then(|r| r.split(']').next())
        .unwrap_or_else(|| panic!("no ref for {needle}"))
        .to_string()
}

#[tokio::main]
async fn main() {
    let base = std::env::args().nth(1).expect("base url");
    let registry = ToolRegistry::new();
    let ctx = ToolContext {
        workspace_dir: std::env::current_dir().unwrap(),
        yolo_mode: true,
        sudo_password: None,
        allowed_commands: Vec::new(),
        extra_dirs: Vec::new(),
    };

    let step = |label: &'static str, args: Value| {
        let (registry, ctx) = (&registry, &ctx);
        async move {
            let out = registry.execute("browser", args, ctx).await.unwrap();
            println!("\n=== {label}{} ===\n{}", if out.is_error { " [ERROR]" } else { "" }, out.output);
            out.output
        }
    };

    let page = step("navigate", json!({"action": "navigate", "url": base})).await;
    step("console", json!({"action": "console"})).await;
    step("type username", json!({"action": "type", "ref": ref_of(&page, "textbox"), "text": "sluisr"})).await;
    step("select plan", json!({"action": "select", "ref": ref_of(&page, "combobox"), "value": "Professional"})).await;
    let after = step("click about", json!({"action": "click", "ref": ref_of(&page, "link \"About")})).await;
    step("back", json!({"action": "back"})).await;
    let form = step("snapshot", json!({"action": "snapshot"})).await;
    step("type username 2", json!({"action": "type", "ref": ref_of(&form, "textbox"), "text": "sluisr"})).await;
    step("select plan 2", json!({"action": "select", "ref": ref_of(&form, "combobox"), "value": "Professional"})).await;
    step("click sign in", json!({"action": "click", "ref": ref_of(&form, "button \"Sign in")})).await;
    step("text", json!({"action": "text"})).await;
    step("screenshot", json!({"action": "screenshot"})).await;
    step("bad ref", json!({"action": "click", "ref": "e999"})).await;
    step("close", json!({"action": "close"})).await;
    let _ = after;
}
