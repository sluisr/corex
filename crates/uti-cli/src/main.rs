use std::path::PathBuf;
use anyhow::Result;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use uti_core::client::{get_sudo_password, LlmClient, StreamEvent};
use uti_core::config::Config;
use uti_core::session::Session;
use uti_core::types::{Message, ToolCall};
use uti_prompt::PromptBuilder;
use uti_tools::registry::ToolRegistry;
use uti_tools::types::ToolContext;
use uti_tui::{run_tui, App};

#[derive(Parser, Debug)]
#[command(
    name = "uti",
    version = env!("CARGO_PKG_VERSION"),
    author = "sluisr <contact@sluisr.com>",
    about = "Universal Terminal Intelligence — High-Performance Autonomous Coding Agent"
)]
struct Cli {
    /// Non-interactive headless message/prompt to execute directly (e.g. -m "..." or -p "...")
    #[arg(short = 'm', short_alias = 'p', long = "message", alias = "prompt")]
    message: Option<String>,

    /// Positional query (if provided without -m/-p, runs headlessly)
    #[arg(trailing_var_arg = true)]
    query: Vec<String>,

    /// Model name override (e.g. --model deepseek-flash, -M deepseek-v4-pro)
    #[arg(short = 'M', long = "model")]
    model: Option<String>,

    /// Custom API base URL (e.g. https://api.deepseek.com)
    #[arg(long)]
    base_url: Option<String>,

    /// API Key override
    #[arg(long)]
    api_key: Option<String>,

    /// Resume a previously saved conversation session by ID, tag, or index
    #[arg(short = 'r', long)]
    resume: Option<String>,

    /// List available conversation sessions for this project
    #[arg(short = 'l', long = "list-sessions")]
    list_sessions: bool,

    /// Automatically approve all tool executions (YOLO mode)
    #[arg(short = 'y', long)]
    yolo: bool,

    /// Working directory (defaults to current working directory)
    #[arg(short = 'C', long)]
    directory: Option<PathBuf>,

    /// Enable or disable local LLM assistant / hybrid mode (--local, --no-local)
    #[arg(long, default_missing_value = "true", num_args = 0..=1)]
    local: Option<bool>,

    /// Custom Local LLM URL (default: http://127.0.0.1:8080/v1)
    #[arg(long)]
    local_url: Option<String>,

    /// Custom Local LLM model name
    #[arg(long)]
    local_model: Option<String>,

    /// Disable hybrid compression of large tool outputs
    #[arg(long)]
    no_hybrid_compression: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let workspace_dir = cli
        .directory
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Handle --list-sessions directly
    if cli.list_sessions {
        let ws_str = workspace_dir.display().to_string();
        let sessions = Session::list_all(Some(&ws_str));
        if sessions.is_empty() {
            println!("No previous sessions found for this project.");
        } else {
            println!("\nAvailable sessions for this project ({}):", sessions.len());
            for (i, s) in sessions.iter().enumerate() {
                let rel_time = Session::format_relative_time(s.updated_at);
                let tag_str = s.tag.as_ref().map(|t| format!(" [tag: {}]", t)).unwrap_or_default();
                let model_display = if s.model == "deepseek-flash" || s.model == "deepseek-v4.1-flash" || s.model == "deepseek-v4-flash" || s.model == "deepseek-chat" {
                    "DeepSeek-V4.1-Flash"
                } else if s.model == "deepseek-v4-pro" || s.model == "deepseek-reasoner" {
                    "DeepSeek-V4-Pro"
                } else if s.model == "deepseek-v4-flash-vision-exp" {
                    "DeepSeek-V4.1-Flash (Vision)"
                } else {
                    &s.model
                };
                println!("  {}. {} ({}){} · {} [{}]", i + 1, s.title, rel_time, tag_str, model_display, s.id);
            }
            println!("\nResume any session with: uti --resume <index | tag | id>\n");
        }
        return Ok(());
    }

    // Handle "uti update" command directly
    if cli.message.is_none() && cli.query.len() == 1 && (cli.query[0] == "update" || cli.query[0] == "--update") {
        let current = env!("CARGO_PKG_VERSION");
        println!("Checking for UTI CLI updates...");
        if let Some(newer) = uti_core::update::check_for_update_online(current).await {
            println!("\n⚡ Update available: v{} → v{}\n", current, newer);
            println!("To update UTI CLI, run in your terminal:");
            println!("  • Via npm:       npm install -g uti-cli");
            println!("  • From source:   cargo install --git https://github.com/sluisr/uti-cli.git --force");
            println!("  • Or download precompiled binaries from:");
            println!("    https://github.com/sluisr/uti-cli/releases/latest\n");
        } else {
            println!("✓ UTI CLI is already on the latest version (v{}).", current);
        }
        return Ok(());
    }

    // Initialize forensic audit log system (~/.uti/logs/uti-forensic-YYYY-MM-DD.log)
    let log_path = uti_core::ForensicLogger::init(Some(&workspace_dir));
    tracing::debug!("Forensic audit logger initialized at {:?}", log_path);

    let mut config = Config::load_with_workspace(Some(&workspace_dir));
    let mut headless_prompt = cli.message;

    if let Some(m) = cli.model {
        // Smart fallback: If a sentence with spaces or question mark was passed to --model, treat as headless prompt!
        if (m.contains(' ') || m.ends_with('?') || m.starts_with("arregla") || m.starts_with("que ") || m.starts_with("fix"))
            && headless_prompt.is_none()
        {
            headless_prompt = Some(m);
        } else {
            config.model = m;
        }
    }
    if let Some(u) = cli.base_url {
        config.base_url = u;
    }
    if let Some(k) = cli.api_key {
        config.api_key = k;
    }
    if let Some(loc_en) = cli.local {
        config.local_llm_enabled = loc_en;
    }
    if let Some(loc_url) = cli.local_url {
        config.local_llm_url = loc_url;
    }
    if let Some(loc_m) = cli.local_model {
        config.local_llm_model = loc_m;
    }
    if cli.no_hybrid_compression {
        config.hybrid_compression = false;
    }
    if cli.yolo {
        config.yolo_mode = true;
    }

    let llm_client = LlmClient::new(config.clone());

    // Check if headless mode requested via -m, -p, or positional args
    if headless_prompt.is_none() && !cli.query.is_empty() {
        headless_prompt = Some(cli.query.join(" "));
    }

    if let Some(prompt_text) = headless_prompt {
        run_headless(llm_client, workspace_dir, prompt_text, config.yolo_mode).await?;
        return Ok(());
    }

    // Launch interactive TUI
    let mut app = App::new(llm_client, workspace_dir.clone(), cli.yolo);
    let _ = app.reload_mcp_servers().await;
    if let Some(session_id) = cli.resume {
        match Session::load_by_id_or_tag(&session_id) {
            Ok(loaded) => {
                let mut cfg = app.llm_client.get_config();
                cfg.model = loaded.model.clone();
                if let Some(t) = loaded.temperature {
                    cfg.temperature = t;
                }
                if let Some(ref r) = loaded.reasoning_effort {
                    cfg.reasoning_effort = r.clone();
                }
                app.llm_client.update_config(cfg);
                app.session = loaded;
            }
            Err(e) => {
                eprintln!("Warning: Could not resume session {}: {}. Starting new session.", session_id, e);
            }
        }
    }

    run_tui(app).await?;
    Ok(())
}

async fn run_headless(
    client: LlmClient,
    workspace_dir: PathBuf,
    user_prompt: String,
    yolo: bool,
) -> Result<()> {
    let mut tool_registry = ToolRegistry::new();
    let (mcp_tools, _) = uti_tools::load_mcp_servers(&client.get_config().mcp_servers).await;
    for t in mcp_tools {
        tool_registry.register(t);
    }
    let prompt_builder = PromptBuilder::new(&workspace_dir)
        .with_sudo_password(get_sudo_password().is_some());

    let mut messages = vec![
        Message::system(prompt_builder.build()),
        Message::user(user_prompt.clone()),
    ];

    println!("❯ {}", user_prompt);

    let tools = tool_registry.list_definitions();
    let context = ToolContext {
        workspace_dir: workspace_dir.clone(),
        yolo_mode: yolo,
        sudo_password: get_sudo_password(),
        allowed_commands: client.get_config().allowed_commands.clone(),
    };

    use std::io::IsTerminal;
    let is_tty = std::io::stdout().is_terminal();
    let term_width = crossterm::terminal::size().map(|(w, _)| w as usize).unwrap_or(80);

    struct StatusSpinner {
        stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
        handle: Option<tokio::task::JoinHandle<()>>,
    }

    impl StatusSpinner {
        fn start(is_tty: bool, prefix_color: &'static str, message: String) -> Self {
            if !is_tty {
                return Self { stop_tx: None, handle: None };
            }
            let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
            let handle = tokio::spawn(async move {
                let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let mut i = 0;
                loop {
                    print!("\r\x1b[2K{}{}\x1b[0m {}", prefix_color, frames[i], message);
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    i = (i + 1) % frames.len();
                    tokio::select! {
                        _ = &mut rx => break,
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(70)) => {}
                    }
                }
                print!("\r\x1b[2K");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            });
            Self {
                stop_tx: Some(tx),
                handle: Some(handle),
            }
        }

        async fn stop(&mut self) {
            if let Some(tx) = self.stop_tx.take() {
                let _ = tx.send(());
            }
            if let Some(handle) = self.handle.take() {
                let _ = handle.await;
            }
        }
    }

    loop {
        let cancel_token = CancellationToken::new();
        let mut rx = client
            .stream_chat(messages.clone(), Some(tools.clone()), cancel_token.clone())
            .await?;

        let mut current_tool_calls: Vec<ToolCall> = Vec::new();
        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut active_spinner: Option<StatusSpinner> = None;
        let mut current_stage: u8 = 0; // 0 = idle, 1 = reasoning, 2 = content

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ReasoningDelta(delta) => {
                    reasoning_text.push_str(&delta);
                    if current_stage != 1 {
                        if let Some(mut s) = active_spinner.take() {
                            s.stop().await;
                        }
                        active_spinner = Some(StatusSpinner::start(
                            is_tty,
                            "\x1b[90m",
                            "Thinking...".to_string(),
                        ));
                        current_stage = 1;
                    }
                }
                StreamEvent::ContentDelta(delta) => {
                    assistant_text.push_str(&delta);
                    if current_stage != 2 {
                        if let Some(mut s) = active_spinner.take() {
                            s.stop().await;
                        }
                        active_spinner = Some(StatusSpinner::start(
                            is_tty,
                            "\x1b[36m",
                            "Generating response...".to_string(),
                        ));
                        current_stage = 2;
                    }
                }
                StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                    while current_tool_calls.len() <= index {
                        current_tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".to_string(),
                            function: uti_core::types::FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                        });
                    }
                    if let Some(i) = id {
                        current_tool_calls[index].id.push_str(&i);
                    }
                    if let Some(n) = name {
                        current_tool_calls[index].function.name.push_str(&n);
                    }
                    if let Some(a) = arguments {
                        current_tool_calls[index].function.arguments.push_str(&a);
                    }
                }
                StreamEvent::Notice(msg) => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    println!("\n[INFO] {}\n", msg);
                }
                StreamEvent::ContextCompacted { notice, .. } => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    println!("\n[INFO] {}\n", notice);
                }
                StreamEvent::Completed { .. } => {}
                StreamEvent::Error(err) => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    eprintln!("\nError: {}", err);
                    return Ok(());
                }
                _ => {}
            }
        }

        if let Some(mut s) = active_spinner.take() {
            s.stop().await;
        }

        if !assistant_text.trim().is_empty() {
            if is_tty {
                let theme = uti_tui::Theme::default();
                let rendered = uti_tui::render_markdown_to_ansi(&assistant_text, &theme, term_width);
                println!("{}\n", rendered);
            } else {
                println!("{}\n", assistant_text);
            }
        }

        if !current_tool_calls.is_empty() {
            let text_opt = if assistant_text.is_empty() { None } else { Some(assistant_text) };
            let cot_opt = if reasoning_text.is_empty() { None } else { Some(reasoning_text) };

            messages.push(Message::assistant_with_tools(
                text_opt,
                cot_opt,
                current_tool_calls.clone(),
            ));

            for call in &current_tool_calls {
                let args_json = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);

                let (action_label, target_detail) = if call.function.name == "run_shell_command" || call.function.name == "shell" || call.function.name == "run_command" {
                    let cmd = args_json.get("command").and_then(|c| c.as_str()).unwrap_or("").trim();
                    ("Running command", cmd)
                } else if call.function.name == "read_file" {
                    let p = args_json.get("path").or_else(|| args_json.get("file_path")).and_then(|c| c.as_str()).unwrap_or("");
                    ("Reading file", p)
                } else if call.function.name == "write_file" {
                    let p = args_json.get("path").or_else(|| args_json.get("file_path")).and_then(|c| c.as_str()).unwrap_or("");
                    ("Writing file", p)
                } else if call.function.name == "edit" || call.function.name == "apply_patch" {
                    let p = args_json.get("path").or_else(|| args_json.get("file_path")).and_then(|c| c.as_str()).unwrap_or("");
                    ("Editing file", p)
                } else if call.function.name == "glob" || call.function.name == "ls" || call.function.name == "list_directory" {
                    let p = args_json.get("path").or_else(|| args_json.get("dir")).and_then(|c| c.as_str()).unwrap_or(".");
                    ("Exploring directory", p)
                } else {
                    ("Running tool", call.function.name.as_str())
                };

                let display_detail = uti_core::truncate_ellipsis(target_detail, 70);

                // Prompt user for confirmation on potentially mutating/dangerous actions unless YOLO mode is enabled
                if !context.yolo_mode {
                    if let Some(tool) = tool_registry.get(&call.function.name) {
                        if tool.needs_confirmation(&args_json, &context) {
                            println!("\n[WARNING] Action requires confirmation: [{}]", call.function.name);
                            if let Some(diff) = tool.format_diff(&args_json, &context.workspace_dir) {
                                println!("{}", diff);
                            } else if let Some(cmd) = args_json.get("command").and_then(|c| c.as_str()) {
                                println!("  Command: {}", cmd);
                            } else {
                                println!("  Arguments: {}", serde_json::to_string_pretty(&args_json).unwrap_or_default());
                            }

                            print!("Allow execution? [y/N]: ");
                            use std::io::Write;
                            let _ = std::io::stdout().flush();

                            let mut input = String::new();
                            let _ = std::io::stdin().read_line(&mut input);
                            let trimmed = input.trim().to_lowercase();
                            if trimmed != "y" && trimmed != "yes" {
                                println!("Execution cancelled by user.");
                                messages.push(Message::tool_response(call.id.clone(), "Execution denied by user."));
                                continue;
                            }
                        }
                    }
                }

                // Show real-time animated spinner while the tool executes
                let mut tool_spinner = StatusSpinner::start(
                    is_tty,
                    "\x1b[33m",
                    format!("\x1b[1m{}:\x1b[0m \x1b[36m{}\x1b[0m", action_label, display_detail),
                );

                let output = match tool_registry
                    .execute(&call.function.name, args_json, &context)
                    .await
                {
                    Ok(o) => {
                        tool_spinner.stop().await;
                        if is_tty {
                            println!("\x1b[32m✓\x1b[0m \x1b[1m{}\x1b[0m: \x1b[90m{}\x1b[0m", action_label, display_detail);
                        }
                        o.output
                    }
                    Err(e) => {
                        tool_spinner.stop().await;
                        if is_tty {
                            println!("\x1b[31m✗\x1b[0m \x1b[1m{}\x1b[0m: \x1b[90m{}\x1b[0m ({})", action_label, display_detail, e);
                        }
                        let msg = format!("Error executing {}: {}", call.function.name, e);
                        eprintln!("{}", msg);
                        msg
                    }
                };
                messages.push(Message::tool_response(call.id.clone(), output));
            }

            // Feed the tool results back and continue the agent loop
            continue;
        }

        // No tool calls: the assistant's final answer has been fully streamed
        break;
    }

    Ok(())
}