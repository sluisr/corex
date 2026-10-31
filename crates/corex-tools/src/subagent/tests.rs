use super::*;
use super::runner::*;
use corex_core::client::StreamEvent;
use corex_core::types::{Message, MessageContent, ToolDefinition};
use crate::registry::ToolRegistry;
use std::path::PathBuf;

#[test]
fn delegating_a_worker_or_tester_needs_approval() {
    let mut ctx = ToolContext {
        workspace_dir: PathBuf::from("/tmp"),
        yolo_mode: false,
        sudo_password: None,
        allowed_commands: Vec::new(),
        extra_dirs: Vec::new(),
    };
    let asks = |role: &str, ctx: &ToolContext| {
        DelegateSubagentTool.needs_confirmation(&json!({ "role": role, "prompt": "x" }), ctx)
    };
    assert!(asks("worker", &ctx));
    assert!(asks("tester", &ctx));
    assert!(asks("anything-else", &ctx), "an unknown role runs as a worker");
    assert!(!asks("scout", &ctx));
    assert!(!asks("researcher", &ctx));

    ctx.allowed_commands.push("delegate_subagent".to_string());
    assert!(!asks("worker", &ctx));
    ctx.allowed_commands.clear();
    ctx.yolo_mode = true;
    assert!(!asks("worker", &ctx));
}

#[tokio::test]
async fn test_delegate_and_manage_subagents() {
    let delegate_tool = DelegateSubagentTool;
    let manage_tool = ManageSubagentsTool;
    let ctx = ToolContext {
        workspace_dir: PathBuf::from("/tmp"),
        yolo_mode: false,
        sudo_password: None,
        allowed_commands: Vec::new(),
        extra_dirs: Vec::new(),
    };

    // 1. Dispatch
    let res = delegate_tool.execute(json!({
        "role": "scout",
        "prompt": "Scan src/ for unsafe blocks"
    }), &ctx).await.expect("tool execution should succeed");

    assert!(!res.is_error);
    assert!(res.output.contains("Task ID: sub_"));

    // Extract ID
    let id_line = res.output.lines().find(|l| l.starts_with("Task ID: ")).unwrap();
    let task_id = id_line.strip_prefix("Task ID: ").unwrap().trim();

    // 2. List
    let list_res = manage_tool.execute(json!({
        "action": "list"
    }), &ctx).await.expect("list should succeed");
    assert!(list_res.output.contains(task_id));

    // 3. Status
    let status_res = manage_tool.execute(json!({
        "action": "status",
        "task_id": task_id
    }), &ctx).await.expect("status should succeed");
    assert!(status_res.output.contains("SCOUT"));

    // 4. Cancel
    let cancel_res = manage_tool.execute(json!({
        "action": "cancel",
        "task_id": task_id
    }), &ctx).await.expect("cancel should succeed");
    assert!(cancel_res.output.contains("cancelled successfully"));
}

#[tokio::test]
async fn test_delegate_subagent_sync_execution() {
    let delegate_tool = DelegateSubagentTool;
    let ctx = ToolContext {
        workspace_dir: PathBuf::from("/tmp"),
        yolo_mode: false,
        sudo_password: None,
        allowed_commands: Vec::new(),
        extra_dirs: Vec::new(),
    };

    let res = delegate_tool.execute(json!({
        "role": "worker",
        "prompt": "echo 'Subagent worker ran'",
        "background": false
    }), &ctx).await.expect("sync execution should succeed");

    assert!(!res.is_error);
    assert!(res.output.contains("completed"));
    assert!(res.output.contains(OFFLINE_REPORT));
}

#[test]
fn test_budget_nudge_is_user_message_with_wrapup_instruction() {
    // (A) The penultimate-turn nudge must be a user message steering the model to wrap up.
    let msg = build_budget_nudge();
    assert_eq!(msg.role, "user");
    let text = match msg.content {
        Some(MessageContent::Text(t)) => t,
        _ => panic!("budget nudge must carry text content"),
    };
    assert!(text.contains("budget"));
    assert!(text.to_lowercase().contains("do not request further tools"));
    assert!(text.contains("final report"));
}

#[test]
fn test_forced_synthesis_prompt_is_user_message_without_tools() {
    // (B) The forced-synthesis prompt must instruct the model to answer without tools.
    let msg = build_forced_synthesis_prompt();
    assert_eq!(msg.role, "user");
    let text = match msg.content {
        Some(MessageContent::Text(t)) => t,
        _ => panic!("forced synthesis prompt must carry text content"),
    };
    assert!(text.contains("maximum number of tool turns"));
    assert!(text.contains("no longer call any tools"));
    assert!(text.contains("final structured"));
}

#[test]
fn test_resolve_exhausted_budget_preserves_gathered_text() {
    // (C) Regression: when the model gathered text, the fallback must return it VERBATIM.
    // The old code discarded the whole investigation and returned a generic string.
    let gathered = "Report: 66 files, 34051 LOC, 6 crates, 238 tests OK.";
    let out = resolve_exhausted_budget(gathered);
    assert_eq!(out, gathered);
    assert_ne!(out, DEFAULT_TURN_LIMIT_MESSAGE);
}

#[test]
fn test_resolve_exhausted_budget_falls_back_only_when_empty() {
    // (C) Only genuinely empty gathers degrade to the generic notice.
    assert_eq!(resolve_exhausted_budget("   \n\t "), DEFAULT_TURN_LIMIT_MESSAGE);
    assert_eq!(resolve_exhausted_budget(""), DEFAULT_TURN_LIMIT_MESSAGE);
    assert!(DEFAULT_TURN_LIMIT_MESSAGE.contains("maximum turn limit"));
}

#[test]
fn test_default_max_turns_are_scoped_per_role() {
    // (D) Role budgets are no longer a single flat 12. Narrowly scoped roles get a tight
    // budget; exploratory roles that legitimately wander get a wider one.
    assert_eq!(default_max_turns_for_role(SubagentRole::Tester), 10);
    assert_eq!(default_max_turns_for_role(SubagentRole::Worker), 16);
    assert_eq!(default_max_turns_for_role(SubagentRole::Researcher), 20);
    assert_eq!(default_max_turns_for_role(SubagentRole::Scout), 24);
    // Tester < Worker < Researcher < Scout.
    assert!(
        default_max_turns_for_role(SubagentRole::Tester)
            < default_max_turns_for_role(SubagentRole::Worker)
    );
    assert!(
        default_max_turns_for_role(SubagentRole::Worker)
            < default_max_turns_for_role(SubagentRole::Researcher)
    );
    assert!(
        default_max_turns_for_role(SubagentRole::Researcher)
            < default_max_turns_for_role(SubagentRole::Scout)
    );
}

#[test]
fn test_max_turns_env_override_parsing() {
    // (D) The `COREX_SUBAGENT_MAX_TURNS` override is parsed race-free via a pure helper,
    // so this test never mutates process-global state.
    assert_eq!(parse_turn_override(Some("3")), Some(3));
    assert_eq!(parse_turn_override(Some("  7 ")), Some(7));
    // Non-positive, signed, empty or garbage values degrade to "no override".
    assert_eq!(parse_turn_override(Some("0")), None);
    assert_eq!(parse_turn_override(Some("-4")), None);
    assert_eq!(parse_turn_override(Some("")), None);
    assert_eq!(parse_turn_override(Some("not-a-number")), None);
    assert_eq!(parse_turn_override(None), None);
}

/// Collects the plain-text content of a message (if any), ignoring non-text parts.
fn message_text(m: &Message) -> String {
    match &m.content {
        Some(MessageContent::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// One recorded call received by the mock backend.
struct MockCall {
    tools_enabled: bool,
    messages: Vec<Message>,
}

/// Deterministic mock chat backend. It always replies with a (harmless) tool call while tools
/// are enabled, and with a textual report once tools are withdrawn — exactly the pathological
/// behaviour that used to kill the old loop with "Subagent reached maximum turn limit".
struct MockChat {
    calls: std::sync::Mutex<Vec<MockCall>>,
}

#[async_trait]
impl SubagentChatBackend for MockChat {
    async fn stream_turn(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, String> {
        self.calls.lock().unwrap().push(MockCall {
            tools_enabled: tools.is_some(),
            messages: messages.clone(),
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let is_synthesis = tools.is_none();
        tokio::spawn(async move {
            if is_synthesis {
                let _ = tx
                    .send(StreamEvent::ContentDelta(
                        "FINAL SYNTHESIZED REPORT: 66 files, 6 crates, 34051 LOC.".to_string(),
                    ))
                    .await;
            } else {
                // Unknown tool -> registry returns an error output, no side effects.
                let _ = tx
                    .send(StreamEvent::ToolCallDelta {
                        index: 0,
                        id: Some("call_mock_1".to_string()),
                        name: Some("__mock_noop_tool__".to_string()),
                        arguments: Some("{}".to_string()),
                        extra_content: None,
                    })
                    .await;
            }
        });
        Ok(rx)
    }
}

/// Plays a fixed script: each turn is the list of tool calls the "model" makes, then a final
/// text once the script runs out. Every request is kept, so a test can read the tool results
/// the subagent fed back to the model.
struct ScriptedChat {
    turns: std::sync::Mutex<std::collections::VecDeque<Vec<(&'static str, String)>>>,
    seen: std::sync::Mutex<Vec<Vec<Message>>>,
}

impl ScriptedChat {
    fn new(turns: Vec<Vec<(&'static str, String)>>) -> Self {
        Self { turns: std::sync::Mutex::new(turns.into()), seen: std::sync::Mutex::new(Vec::new()) }
    }

    /// Tool results the subagent sent back, in order.
    fn tool_results(&self) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        let last = seen.last().cloned().unwrap_or_default();
        last.iter().filter(|m| m.role == "tool").map(message_text).collect()
    }
}

#[async_trait]
impl SubagentChatBackend for ScriptedChat {
    async fn stream_turn(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, String> {
        self.seen.lock().unwrap().push(messages);
        let next = if tools.is_some() { self.turns.lock().unwrap().pop_front() } else { None };
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(32);
        tokio::spawn(async move {
            match next {
                Some(calls) => {
                    for (i, (name, args)) in calls.into_iter().enumerate() {
                        let _ = tx
                            .send(StreamEvent::ToolCallDelta {
                                index: i,
                                id: Some(format!("call_{i}_{name}")),
                                name: Some(name.to_string()),
                                arguments: Some(args),
                                extra_content: None,
                            })
                            .await;
                    }
                }
                None => {
                    let _ = tx.send(StreamEvent::ContentDelta("SCRIPT DONE".to_string())).await;
                }
            }
            let _ = tx.send(StreamEvent::Completed { finish_reason: None }).await;
        });
        Ok(rx)
    }
}

async fn run_script(role: SubagentRole, ws: &Path, turns: Vec<Vec<(&'static str, String)>>) -> (String, Vec<String>) {
    let chat = ScriptedChat::new(turns);
    let mut trace = ForensicTrace::new("sub_script", role, "script", ws);
    let out = run_subagent_llm_loop_with("sub_script", role, "script", ws, &[], &[], &chat, &mut trace)
        .await
        .unwrap_or_else(|e| format!("ERR: {e}"));
    (out, chat.tool_results())
}

fn script_ws(name: &str) -> PathBuf {
    let ws = std::env::temp_dir().join(format!("corex_script_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();
    ws
}

#[tokio::test]
async fn wait_returns_reports_once_subagents_finish() {
    let id = get_subagent_manager().lock().unwrap().register(SubagentRole::Scout, "wait test".into());
    let quick = wait_for_subagents(Some(&id), std::time::Duration::from_secs(1)).await;
    assert!(quick.output.contains("still running"), "{}", quick.output);

    let finisher = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        get_subagent_manager().lock().unwrap().complete(&finisher, "MAPPED 3 FILES".into());
    });
    let started = std::time::Instant::now();
    let done = wait_for_subagents(Some(&id), std::time::Duration::from_secs(20)).await;
    assert!(done.output.contains("All watched subagents have finished"), "{}", done.output);
    assert!(done.output.contains("MAPPED 3 FILES"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "must return as soon as it finishes");

    let missing = wait_for_subagents(Some("sub_nope"), std::time::Duration::from_secs(1)).await;
    assert!(missing.is_error);
}

#[tokio::test]
async fn real_loop_worker_reads_edits_patches_and_runs() {
    let ws = script_ws("worker");
    let patch = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,4 @@\n pub fn add(a: i32, b: i32) -> i32 {\n     a.wrapping_add(b)\n }\n+pub fn two() -> i32 { 2 }\n";
    let (out, results) = run_script(
        SubagentRole::Worker,
        &ws,
        vec![
            vec![("read_file", json!({"file_path": "src/lib.rs"}).to_string())],
            vec![("edit", json!({"file_path": "src/lib.rs", "old_string": "a + b", "new_string": "a.wrapping_add(b)"}).to_string())],
            vec![("apply_patch", json!({"patch": patch}).to_string())],
            vec![
                ("write_file", json!({"file_path": "notes/plan.md", "content": "hecho\n"}).to_string()),
                ("run_shell_command", json!({"command": "cat src/lib.rs notes/plan.md"}).to_string()),
            ],
        ],
    )
    .await;
    let lib = std::fs::read_to_string(ws.join("src/lib.rs")).unwrap();
    let plan = std::fs::read_to_string(ws.join("notes/plan.md")).ok();
    let _ = std::fs::remove_dir_all(&ws);
    assert!(out.contains("SCRIPT DONE"), "{out}");
    assert!(lib.contains("a.wrapping_add(b)") && lib.contains("pub fn two()"), "{lib}\n{results:#?}");
    assert_eq!(plan.as_deref(), Some("hecho\n"), "{results:#?}");
    assert!(results.last().is_some_and(|r| r.contains("pub fn two()") && r.contains("hecho")), "{results:#?}");
}

#[tokio::test]
async fn real_loop_guards_hold_for_every_role() {
    let ws = script_ws("guards");
    let attempts = || {
        vec![vec![
            ("write_file", json!({"file_path": "src/lib.rs", "content": "pwned"}).to_string()),
            ("run_shell_command", json!({"command": "echo ok\nsudo id"}).to_string()),
            ("run_shell_command", json!({"command": "echo x >~/.bashrc"}).to_string()),
            ("run_shell_command", json!({"command": "ls", "cwd": "/"}).to_string()),
            ("read_file", json!({"file_path": "~/.ssh/id_ed25519"}).to_string()),
            ("write_file", json!({"file_path": "/tmp/corex_script_outside.txt", "content": "x"}).to_string()),
            ("delegate_subagent", json!({"role": "worker", "prompt": "recurse"}).to_string()),
            ("save_memory", json!({"fact": "persist me"}).to_string()),
            ("no_such_tool", "{}".to_string()),
            ("read_file", "{not json".to_string()),
        ]]
    };
    for role in [SubagentRole::Scout, SubagentRole::Researcher, SubagentRole::Tester, SubagentRole::Worker] {
        let (out, results) = run_script(role, &ws, attempts()).await;
        let lib = std::fs::read_to_string(ws.join("src/lib.rs")).unwrap();
        println!("--- {role:?} -> {}", out.lines().next().unwrap_or(""));
        for r in &results {
            println!("    {}", r.lines().next().unwrap_or(""));
        }
        assert!(out.contains("SCRIPT DONE"), "{role:?} must finish: {out}");
        assert!(!std::path::Path::new("/tmp/corex_script_outside.txt").exists(), "{role:?} wrote outside the workspace");
        // Read-only roles have no write tools; the worker may write, but never over a file it
        // has not read in this session.
        assert!(lib.contains("a + b"), "{role:?} overwrote an unread file");
        assert_eq!(results.len(), 10, "{role:?}: every call gets an answer: {results:#?}");
        assert!(!results[1].contains("uid="), "{role:?} ran sudo: {}", results[1]);
        assert!(results[4].contains("denied") || results[4].contains("Unknown tool"), "{role:?} read ~/.ssh: {}", results[4]);
        assert!(!results[6].contains("completed"), "{role:?} spawned a subagent: {}", results[6]);
    }
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_subagent_policy_denies_unapproved_actions() {
    let ws = std::env::temp_dir().join(format!("corex-subpolicy-{}", std::process::id()));
    std::fs::create_dir_all(ws.join("src")).unwrap();
    let reg = ToolRegistry::new();
    let ctx_for = |role: SubagentRole| {
        let mut allowed = Vec::new();
        if role == SubagentRole::Worker {
            allowed.extend(["edit", "write_file", "apply_patch"].map(String::from));
        }
        ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: allowed,
            extra_dirs: Vec::new(),
        }
    };
    let deny = |role, name: &str, args: serde_json::Value| {
        crate::policy::subagent_denial(role, &reg, name, &args, &ctx_for(role)).is_some()
    };
    let sh = |c: &str| serde_json::json!({ "command": c });

    let scout = SubagentRole::Scout;
    assert!(!deny(scout, "read_file", serde_json::json!({"file_path": "src/main.rs"})));
    assert!(deny(scout, "read_file", serde_json::json!({"file_path": "/etc/hostname"})));
    assert!(!deny(scout, "run_shell_command", sh("ls -la")));
    assert!(deny(scout, "run_shell_command", sh("cargo build")));
    assert!(deny(scout, "run_shell_command", sh("sudo ls")));

    let worker = SubagentRole::Worker;
    assert!(!deny(worker, "run_shell_command", sh("cargo build")));
    assert!(deny(worker, "run_shell_command", sh("sudo apt-get update")));
    assert!(!deny(worker, "write_file", serde_json::json!({"file_path": "src/a.rs", "content": "x"})));
    assert!(deny(worker, "write_file", serde_json::json!({"file_path": "/etc/motd", "content": "x"})));
    // Reading a web page needs no approval for anyone, like a search.
    assert!(!deny(worker, "web_fetch", serde_json::json!({"url": "https://example.org"})));
    assert!(!deny(scout, "web_fetch", serde_json::json!({"url": "https://example.org"})));
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn test_llm_loop_forces_synthesis_after_turn_limit() {
    // Regression for the turn-limit bug: a subagent that NEVER yields a tool-free turn must
    // still return a real report thanks to (A) budget nudge + (B) forced synthesis, instead
    // of discarding the whole investigation with a generic string.
    let mock = MockChat {
        calls: std::sync::Mutex::new(Vec::new()),
    };
    let workspace = std::env::temp_dir();
    let mut trace = ForensicTrace::new(
        "sub_test_turn_limit",
        SubagentRole::Scout,
        "map the repo",
        &workspace,
    );

    let result = run_subagent_llm_loop_with(
        "sub_test_turn_limit",
        SubagentRole::Scout,
        "map the repo",
        &workspace,
        &[],
        &[],
        &mock,
        &mut trace,
    )
    .await;

    let out = result.expect("loop must complete successfully");
    assert!(
        out.contains("FINAL SYNTHESIZED REPORT"),
        "forced synthesis must be returned, got: {}",
        out
    );
    assert_ne!(out, DEFAULT_TURN_LIMIT_MESSAGE);

    let calls = mock.calls.lock().unwrap();
    let expected_turns = max_turns_for_role(SubagentRole::Scout);
    assert_eq!(
        calls.len(),
        expected_turns + 1,
        "expected {} tool turns + 1 forced tool-less synthesis call",
        expected_turns
    );
    assert!(
        !calls.last().unwrap().tools_enabled,
        "the final call must be tool-less (forced synthesis, fix B)"
    );
    let nudge_seen = calls
        .iter()
        .any(|c| c.messages.iter().any(|m| message_text(m).contains("Turn budget notice")));
    assert!(
        nudge_seen,
        "the penultimate turn must carry the budget nudge (fix A)"
    );
}

#[tokio::test]
async fn test_subagent_forensic_log_generation_and_query() {
    let delegate_tool = DelegateSubagentTool;
    let manage_tool = ManageSubagentsTool;
    let test_id = format!("corex_forensic_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let temp_dir = std::env::temp_dir().join(test_id);
    std::fs::create_dir_all(&temp_dir).expect("create_dir");

    let ctx = ToolContext {
        workspace_dir: temp_dir.clone(),
        yolo_mode: false,
        sudo_password: None,
        allowed_commands: Vec::new(),
        extra_dirs: Vec::new(),
    };

    let res = delegate_tool.execute(json!({
        "role": "worker",
        "prompt": "echo 'Testing forensic trace audit'",
        "background": false
    }), &ctx).await.expect("worker execution should succeed");

    assert!(!res.is_error);

    // Find the task ID from synchronous execution output ("Subagent 'sub_...' completed:")
    let task_id = res.output
        .strip_prefix("Subagent '")
        .and_then(|s| s.split('\'').next())
        .expect("should find task id");

    // Query status -> must include Log File
    let status_res = manage_tool.execute(json!({
        "action": "status",
        "task_id": task_id
    }), &ctx).await.expect("status should succeed");

    assert!(status_res.output.contains("Log File: "));
    assert!(status_res.output.contains(".corex/logs/subagents"));

    // Query log -> must return full forensic audit log
    let log_res = manage_tool.execute(json!({
        "action": "log",
        "task_id": task_id
    }), &ctx).await.expect("log action should succeed");

    assert!(!log_res.is_error);
    assert!(log_res.output.contains("--- SUBAGENT EXECUTION LOG ---"));
    assert!(log_res.output.contains("--- PROMPT ---"));
    assert!(log_res.output.contains("--- STEPS ---"));
    assert!(log_res.output.contains("Testing forensic trace audit"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}
