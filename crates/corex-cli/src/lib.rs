use std::path::PathBuf;
use anyhow::Result;
use clap::{CommandFactory, Parser};
use tokio_util::sync::CancellationToken;

use corex_core::client::{get_sudo_password, LlmClient, StreamEvent};
use corex_core::config::Config;
use corex_core::session::Session;
use corex_core::types::{Message, ToolCall};
use corex_prompt::PromptBuilder;
use corex_tools::registry::ToolRegistry;
use corex_tools::types::ToolContext;
use corex_tui::{run_tui, App};

mod fleet_cli;

#[derive(Parser, Debug, PartialEq)]
#[command(
    name = "cx",
    version = env!("CARGO_PKG_VERSION"),
    author = "sluisr <contact@sluisr.com>",
    about = "Corex — High-Performance Autonomous Coding Agent"
)]
struct Cli {
    /// Non-interactive headless prompt to execute directly (e.g. -p "..." or --prompt "...")
    #[arg(short = 'p', long = "prompt", alias = "message")]
    message: Option<String>,

    /// Positional query (if provided without -p, runs headlessly)
    #[arg(trailing_var_arg = true)]
    query: Vec<String>,

    /// Perform live web search (e.g. -w "query" or --web "query")
    #[arg(short = 'w', long = "web")]
    web: Option<String>,

    /// Fetch and summarize a web page URL as Markdown (e.g. --fetch "https://...")
    #[arg(long = "fetch")]
    fetch: Option<String>,

    /// Model name override (e.g. -m deepseek-flash, --model deepseek-v4-pro)
    #[arg(short = 'm', short_alias = 'M', long = "model")]
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

    /// Enable or disable local LLM assistant (--local, --no-local)
    #[arg(long, default_missing_value = "true", num_args = 0..=1)]
    local: Option<bool>,

    /// Custom Local LLM URL (default: http://127.0.0.1:8080/v1)
    #[arg(long)]
    local_url: Option<String>,

    /// Custom Local LLM model name
    #[arg(long)]
    local_model: Option<String>,

    /// Update Corex to the latest release in-place (alias: --upgrade)
    #[arg(long = "upgrade", alias = "update")]
    upgrade: bool,

    /// Skip the startup check for a newer release (also: COREX_NO_UPDATE_CHECK=1)
    #[arg(long = "no-update-check")]
    no_update_check: bool,

    /// Force update or reinstallation even if already on the latest version
    #[arg(long = "force", short = 'f')]
    force: bool,

    /// Generate shell autocompletion script (bash, zsh, fish, powershell, elvish)
    #[arg(long = "completions", value_enum)]
    completions: Option<clap_complete::Shell>,

    /// Manage remote VPS servers and clusters (SysAdmin operations)
    #[arg(long = "fleet", alias = "sysadmin")]
    fleet: bool,
}

pub async fn run() -> Result<()> {
    let mut cli = Cli::parse();
    if let Some(arg0) = std::env::args().next() {
        if (arg0.ends_with("cxs") || arg0.ends_with("cx-fleet")) && !cli.fleet {
            cli.fleet = true;
        }
    }
    if cli.no_update_check {
        corex_core::update::disable_update_check();
    }

    let workspace_dir = cli
        .directory
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

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
                } else if s.model == "deepseek-v4.1-flash-1017" {
                    "DeepSeek-V4.1-Flash-1017"
                } else if s.model == "deepseek-v4-pro" || s.model == "deepseek-reasoner" {
                    "DeepSeek-V4-Pro"
                } else if s.model == "deepseek-v4-flash-vision-exp" {
                    "DeepSeek-V4.1-Flash (Vision)"
                } else {
                    &s.model
                };
                println!("  {}. {} ({}){} · {} [{}]", i + 1, s.title, rel_time, tag_str, model_display, s.id);
            }
            println!("\nResume any session with: cx --resume <index | tag | id>\n");
        }
        return Ok(());
    }

    let completions_shell = cli.completions.or_else(|| {
        if cli.message.is_none() && !cli.query.is_empty() && cli.query[0] == "completions" {
            cli.query.get(1).and_then(|s| s.parse::<clap_complete::Shell>().ok())
        } else {
            None
        }
    });

    if let Some(shell) = completions_shell {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "cx", &mut std::io::stdout());
        return Ok(());
    }

    if cli.message.is_none() && !cli.query.is_empty() && cli.query[0] == "completions" && cli.query.len() == 1 {
        println!("Usage: cx --completions <bash|zsh|fish|powershell|elvish>");
        return Ok(());
    }

    let is_fleet_cmd = cli.fleet
        || (cli.message.is_none()
            && !cli.query.is_empty()
            && (cli.query[0] == "fleet" || cli.query[0] == "sysadmin"));

    if is_fleet_cmd {
        cli.fleet = true;
        let fleet_args: Vec<String> = if !cli.query.is_empty() && (cli.query[0] == "fleet" || cli.query[0] == "sysadmin") {
            cli.query[1..].to_vec()
        } else {
            cli.query.clone()
        };

        let is_headless_subcommand = fleet_args.first().map(|s| {
            matches!(
                s.as_str(),
                "status" | "list" | "ls" | "nodes" | "ping" | "top" | "telemetry" | "metrics" | "init" | "run" | "broadcast" | "all" | "help" | "--help" | "-h"
            )
        }).unwrap_or(false);

        if is_headless_subcommand {
            return fleet_cli::handle_fleet_cli(&workspace_dir, &fleet_args).await;
        }

        if !cli.query.is_empty() && (cli.query[0] == "fleet" || cli.query[0] == "sysadmin") {
            cli.query.remove(0);
        }
    }

    let is_update_cmd = cli.upgrade
        || (cli.message.is_none()
            && !cli.query.is_empty()
            && (cli.query[0] == "update" || cli.query[0] == "upgrade" || cli.query[0] == "--update" || cli.query[0] == "--upgrade"));

    if is_update_cmd {
        let current = env!("CARGO_PKG_VERSION");
        let force = cli.force || cli.query.iter().any(|q| q == "--force" || q == "-f");
        println!("Corex In-Place Self-Updater (v{})", current);
        match corex_core::update::perform_self_update(current, force, |msg| {
            println!("  › {}", msg);
        }).await {
            Ok(summary) => {
                println!(
                    "\n✓ Successfully updated Corex from v{} to v{}!\n  Binary: {}\n  Target: {}\n",
                    summary.previous_version,
                    summary.updated_version,
                    summary.binary_path.display(),
                    summary.target_triple
                );
            }
            Err(e) => {
                eprintln!("\nNotice: {}", e);
                if !force && e.contains("already on the latest version") {
                    println!("To force re-download the latest release, run:\n  cx update --force\n");
                }
            }
        }
        return Ok(());
    }

    let log_path = corex_core::ForensicLogger::init(Some(&workspace_dir));
    tracing::debug!("Session logger initialized at {:?}", log_path);

    let mut config = Config::load_with_workspace(Some(&workspace_dir));
    let mut headless_prompt = cli.message.clone();
    let mut explicit_model = false;

    if let Some(m) = cli.model {
        // Multi-word values passed to `-m` are treated as a headless prompt when `-p` is absent.
        if m.contains(' ') && headless_prompt.is_none() {
            headless_prompt = Some(m);
        } else {
            config.model = m;
            explicit_model = true;
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
    if cli.yolo {
        config.yolo_mode = true;
    }

    let mut resumed_session: Option<Session> = None;
    if let Some(ref session_id) = cli.resume {
        match Session::load_by_id_or_tag(session_id) {
            Ok(loaded) => {
                if !explicit_model {
                    config.model = loaded.model.clone();
                }
                if let Some(t) = loaded.temperature {
                    config.temperature = t;
                }
                if let Some(ref r) = loaded.reasoning_effort {
                    config.reasoning_effort = r.clone();
                }
                resumed_session = Some(loaded);
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not resume session {}: {}. Starting new session.",
                    session_id, e
                );
            }
        }
    }

    let llm_client = LlmClient::new(config.clone());
    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(llm_client.clone()));

    let make_session = |resumed: Option<Session>| -> Session {
        resumed.unwrap_or_else(|| {
            Session::new_with_params(
                &config.model,
                config.temperature,
                &config.reasoning_effort,
                Some(&workspace_dir),
            )
        })
    };

    let web_search_query = cli.web.or_else(|| {
        if cli.message.is_none() && !cli.query.is_empty() && (cli.query[0] == "web" || cli.query[0] == "/web" || cli.query[0] == "search" || cli.query[0] == "/search") {
            let q = cli.query[1..].join(" ");
            if !q.is_empty() {
                Some(q)
            } else {
                None
            }
        } else {
            None
        }
    });

    if let Some(query) = web_search_query {
        let prompt = format!(
            "Please search the web using the web_search tool and synthesize a clear, direct, and well-structured answer with source links for: {}",
            query
        );
        let display = format!("/web {}", query);
        let session = make_session(resumed_session);
        run_headless(llm_client, workspace_dir, prompt, Some(display), config.yolo_mode, session).await?;
        return Ok(());
    }

    let web_fetch_url = cli.fetch.or_else(|| {
        if cli.message.is_none() && !cli.query.is_empty() && (cli.query[0] == "fetch" || cli.query[0] == "/fetch") {
            let u = cli.query[1..].join(" ");
            if !u.is_empty() {
                Some(u)
            } else {
                None
            }
        } else {
            None
        }
    });

    if let Some(url) = web_fetch_url {
        let prompt = format!(
            "Please fetch the URL '{}' using the web_fetch tool and synthesize a clear, comprehensive summary and analysis of its content.",
            url
        );
        let display = format!("/fetch {}", url);
        let session = make_session(resumed_session);
        run_headless(llm_client, workspace_dir, prompt, Some(display), config.yolo_mode, session).await?;
        return Ok(());
    }

    if headless_prompt.is_none() && !cli.query.is_empty() {
        headless_prompt = Some(cli.query.join(" "));
    }

    if let Some(prompt_text) = headless_prompt {
        let session = make_session(resumed_session);
        run_headless(llm_client, workspace_dir, prompt_text, None, config.yolo_mode, session).await?;
        return Ok(());
    }

    let mut app = App::new(llm_client, workspace_dir.clone(), cli.yolo);
    let _ = app.reload_mcp_servers().await;
    if cli.fleet {
        app.enter_fleet_mode();
        app.fleet_dialog.open_async(&workspace_dir).await;
    }
    if let Some(loaded) = resumed_session {
        app.session = loaded;
    }

    run_tui(app).await?;
    Ok(())
}

async fn run_headless(
    client: LlmClient,
    workspace_dir: PathBuf,
    user_prompt: String,
    display_prompt: Option<String>,
    yolo: bool,
    mut session: Session,
) -> Result<()> {
    let mut tool_registry = ToolRegistry::new();
    corex_fleet::register_fleet_tools(&mut tool_registry);
    let (mcp_tools, _) = corex_tools::load_mcp_servers(&client.get_config().mcp_servers).await;
    for t in mcp_tools {
        tool_registry.register(t);
    }
    let prompt_builder = PromptBuilder::new(&workspace_dir)
        .with_sudo_password(get_sudo_password().is_some());

    session.add_message(Message::user(user_prompt.clone()));

    let mut messages = Vec::with_capacity(session.messages.len() + 1);
    messages.push(Message::system(prompt_builder.build()));
    for m in &session.messages {
        let is_preserved_system = m.role == "system"
            && m.content.as_ref().and_then(|c| c.as_text()).map_or(false, |c| {
                c.contains("<CONTEXT_SUMMARY>")
                    || c.starts_with("TASK_DONE|")
                    || c.starts_with("SUBAGENT_DONE|")
            });
        if m.role != "system" || is_preserved_system {
            messages.push(m.clone());
        }
    }

    use colored::Colorize;
    let you_badge = format!(
        "{}{}{}",
        "[".truecolor(95, 95, 95),
        "You".bold().truecolor(130, 215, 160),
        "]".truecolor(95, 95, 95),
    );
    println!("{} {}", you_badge, display_prompt.as_deref().unwrap_or(&user_prompt));

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
            print!("\r\x1b[2K");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    }

    const MAX_HEADLESS_TURNS: usize = 35;
    let mut turn_count = 0;
    let mut truncated_retries = 0u8;

    loop {
        turn_count += 1;
        if turn_count > MAX_HEADLESS_TURNS {
            eprintln!("\n[WARNING] Maximum headless tool iteration limit ({}) reached. Terminating loop to protect budget.", MAX_HEADLESS_TURNS);
            break;
        }

        let cancel_token = CancellationToken::new();
        let mut rx = client
            .stream_chat(messages.clone(), Some(tools.clone()), cancel_token.clone())
            .await?;

        let mut current_tool_calls: Vec<ToolCall> = Vec::new();
        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut current_extra_content: Option<serde_json::Value> = None;
        let mut active_spinner: Option<StatusSpinner> = None;
        let mut current_stage: u8 = 0; // 0 = idle, 1 = reasoning, 2 = content
        let mut finish_reason: Option<String> = None;

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
                StreamEvent::ToolCallDelta { index, id, name, arguments, extra_content } => {
                    if index > 64 {
                        continue;
                    }
                    while current_tool_calls.len() <= index {
                        current_tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".to_string(),
                            function: corex_core::types::FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                            extra_content: None,
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
                    if let Some(ec) = extra_content {
                        current_tool_calls[index].extra_content = Some(ec);
                    }
                }
                StreamEvent::ExtraContentDelta(ec) => {
                    current_extra_content = Some(ec);
                }
                StreamEvent::Notice(msg) => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    println!("\n[INFO] {}\n", msg);
                }
                StreamEvent::ContextCompacted { compacted_messages, notice } => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    let sys_prompt = messages.first().cloned();
                    let before = std::mem::replace(&mut session.messages, compacted_messages.clone());
                    session.keep_history_before_compaction(&before);
                    messages.clear();
                    if let Some(sys) = sys_prompt {
                        messages.push(sys);
                    }
                    for m in compacted_messages {
                        if m.role != "system"
                            || m.content.as_ref().and_then(|c| c.as_text()).map_or(false, |c| {
                                c.contains("<CONTEXT_SUMMARY>")
                                    || c.starts_with("TASK_DONE|")
                                    || c.starts_with("SUBAGENT_DONE|")
                            })
                        {
                            messages.push(m);
                        }
                    }
                    println!("\n[INFO] {}\n", notice);
                }
                StreamEvent::UsageUpdate(usage) => {
                    session.update_usage(&usage);
                }
                StreamEvent::Completed { finish_reason: reason } => {
                    if reason.is_some() {
                        finish_reason = reason;
                    }
                }
                StreamEvent::Error(err) => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    eprintln!("\nError: {}", err);
                    let _ = session.save();
                    return Ok(());
                }
                _ => {}
            }
        }

        if let Some(mut s) = active_spinner.take() {
            s.stop().await;
        }

        let user_cjk = corex_core::language::user_has_requested_cjk(&messages);
        if corex_core::language::is_unwanted_cjk_drift(&assistant_text, user_cjk) {
            assistant_text = corex_core::language::clean_cjk_drift_from_text(&assistant_text);
        }

        if !assistant_text.trim().is_empty() {
            let safe_text = corex_core::sanitize_for_terminal(&assistant_text);
            if is_tty {
                let theme = corex_tui::Theme::default();
                let rendered = corex_tui::render_markdown_to_ansi(&safe_text, &theme, term_width);
                println!("{}\n", rendered);
            } else {
                println!("{}\n", safe_text);
            }
        }

        // Providers that index tool calls by content block (Anthropic) leave empty placeholders.
        current_tool_calls.retain(|c| !c.function.name.trim().is_empty());

        if !current_tool_calls.is_empty() {
            let text_opt = if assistant_text.is_empty() { None } else { Some(assistant_text) };
            let cot_opt = if reasoning_text.is_empty() { None } else { Some(reasoning_text) };

            for call in &mut current_tool_calls {
                if call.extra_content.is_none() {
                    call.extra_content = current_extra_content.clone();
                }
            }

            let assistant_msg = Message::assistant_with_tools(
                text_opt,
                cot_opt,
                current_tool_calls.clone(),
            );
            messages.push(assistant_msg.clone());
            session.add_message(assistant_msg);

            if corex_core::is_truncated_finish(finish_reason.as_deref()) {
                // Arguments may be cut mid-file or mid-command: never run them.
                for call in &current_tool_calls {
                    let msg = Message::tool_response(call.id.clone(), corex_core::TRUNCATED_TOOL_CALL_ERROR);
                    messages.push(msg.clone());
                    session.add_message(msg);
                }
                truncated_retries += 1;
                if truncated_retries > 1 {
                    eprintln!("\n[WARNING] Response truncated twice by the output token limit; tool calls were not executed.");
                    break;
                }
                eprintln!("\n[WARNING] Response truncated by the output token limit; asking the model to retry in smaller pieces.");
                continue;
            }
            truncated_retries = 0;

            for call in &current_tool_calls {
                let args_json = corex_core::parse_lenient_json(&call.function.arguments)
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
                } else if call.function.name == "web_search" || call.function.name == "google_web_search" {
                    let q = args_json.get("query").and_then(|c| c.as_str()).unwrap_or("");
                    ("Searching web", q)
                } else if call.function.name == "web_fetch" {
                    let u = args_json.get("url").and_then(|c| c.as_str()).unwrap_or("");
                    ("Fetching web page", u)
                } else {
                    ("Running tool", call.function.name.as_str())
                };

                let display_detail = corex_core::truncate_ellipsis(&corex_core::sanitize_for_terminal(target_detail), 70);

                if !context.yolo_mode {
                    if let Some(tool) = tool_registry.get(&call.function.name) {
                        if tool.needs_confirmation(&args_json, &context) {
                            if is_tty {
                                print!("\r\x1b[2K");
                                use std::io::Write;
                                let _ = std::io::stdout().flush();
                            }
                            println!("\n[WARNING] Action requires confirmation: [{}]", call.function.name);
                            if let Some(diff) = tool.format_diff(&args_json, &context.workspace_dir) {
                                println!("{}", corex_core::sanitize_for_terminal(&diff));
                            } else if let Some(cmd) = args_json.get("command").and_then(|c| c.as_str()) {
                                println!("  Command: {}", corex_core::sanitize_for_terminal(cmd));
                            } else {
                                println!("  Arguments: {}", corex_core::sanitize_for_terminal(&serde_json::to_string_pretty(&args_json).unwrap_or_default()));
                            }

                            print!("Allow execution? [y/N]: ");
                            use std::io::Write;
                            let _ = std::io::stdout().flush();

                            let mut input = String::new();
                            let _ = std::io::stdin().read_line(&mut input);
                            let trimmed = input.trim().to_lowercase();
                            if trimmed != "y" && trimmed != "yes" {
                                println!("Execution cancelled by user.");
                                let denied_msg = Message::tool_response(call.id.clone(), "Execution denied by user.");
                                messages.push(denied_msg.clone());
                                session.add_message(denied_msg);
                                continue;
                            }
                        }
                    }
                }

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
                // Huge output is cut to head + tail; the full text stays readable from a file.
                let output = corex_core::spill_large_output(&call.function.name, output);
                // Advisory nudge when the model keeps repeating the exact same call.
                let output = match corex_core::observe_tool_call(&call.function.name, &call.function.arguments) {
                    Some(reminder) => format!("{}\n\n{}", output, reminder),
                    None => output,
                };
                let tool_msg = Message::tool_response(call.id.clone(), output);
                messages.push(tool_msg.clone());
                session.add_message(tool_msg);
            }

            if let Some(image_msg) = corex_core::drain_tool_images_message() {
                messages.push(image_msg.clone());
                session.add_message(image_msg);
            }

            continue;
        }

        if !assistant_text.is_empty() || !reasoning_text.is_empty() {
            let cot_opt = if reasoning_text.is_empty() { None } else { Some(reasoning_text) };
            let final_msg = Message::assistant(assistant_text, cot_opt);
            session.add_message(final_msg);
        }
        break;
    }

    let _ = session.save();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_command_factory_debug_assert() {
        // Validates clap internal consistency, non-colliding flags, and value parsers.
        Cli::command().debug_assert();
    }

    #[test]
    fn test_cli_default_args() {
        let cli = Cli::try_parse_from(["cx"]).expect("Should parse empty invocation");
        assert_eq!(cli.message, None);
        assert!(cli.query.is_empty());
        assert_eq!(cli.model, None);
        assert_eq!(cli.web, None);
        assert_eq!(cli.fetch, None);
        assert!(!cli.yolo);
        assert!(!cli.fleet);
        assert!(!cli.list_sessions);
        assert_eq!(cli.resume, None);
        assert_eq!(cli.local, None);
    }

    #[test]
    fn test_cli_prompt_flags() {
        let short = Cli::try_parse_from(["cx", "-p", "Refactor main.rs"]).unwrap();
        assert_eq!(short.message.as_deref(), Some("Refactor main.rs"));

        let long = Cli::try_parse_from(["cx", "--prompt", "Explain this"]).unwrap();
        assert_eq!(long.message.as_deref(), Some("Explain this"));

        let alias = Cli::try_parse_from(["cx", "--message", "Alias test"]).unwrap();
        assert_eq!(alias.message.as_deref(), Some("Alias test"));
    }

    #[test]
    fn test_cli_model_selection_flags() {
        let short = Cli::try_parse_from(["cx", "-m", "gemini-3.8-flash"]).unwrap();
        assert_eq!(short.model.as_deref(), Some("gemini-3.8-flash"));

        let long = Cli::try_parse_from(["cx", "--model", "claude-3-7-sonnet"]).unwrap();
        assert_eq!(long.model.as_deref(), Some("claude-3-7-sonnet"));

        let short_alias = Cli::try_parse_from(["cx", "-M", "o3-mini"]).unwrap();
        assert_eq!(short_alias.model.as_deref(), Some("o3-mini"));
    }

    #[test]
    fn test_cli_yolo_mode_flags() {
        let short = Cli::try_parse_from(["cx", "-y"]).unwrap();
        assert!(short.yolo);

        let long = Cli::try_parse_from(["cx", "--yolo"]).unwrap();
        assert!(long.yolo);
    }

    #[test]
    fn test_cli_web_and_fetch_flags() {
        let web_short = Cli::try_parse_from(["cx", "-w", "tokio select"]).unwrap();
        assert_eq!(web_short.web.as_deref(), Some("tokio select"));

        let web_long = Cli::try_parse_from(["cx", "--web", "rust 2024 edition"]).unwrap();
        assert_eq!(web_long.web.as_deref(), Some("rust 2024 edition"));

        let fetch = Cli::try_parse_from(["cx", "--fetch", "https://docs.rs"]).unwrap();
        assert_eq!(fetch.fetch.as_deref(), Some("https://docs.rs"));
    }

    #[test]
    fn test_cli_session_management_flags() {
        let list_short = Cli::try_parse_from(["cx", "-l"]).unwrap();
        assert!(list_short.list_sessions);

        let list_long = Cli::try_parse_from(["cx", "--list-sessions"]).unwrap();
        assert!(list_long.list_sessions);

        let resume_short = Cli::try_parse_from(["cx", "-r", "sess_abc123"]).unwrap();
        assert_eq!(resume_short.resume.as_deref(), Some("sess_abc123"));

        let resume_long = Cli::try_parse_from(["cx", "--resume", "tag-refactor"]).unwrap();
        assert_eq!(resume_long.resume.as_deref(), Some("tag-refactor"));
    }

    #[test]
    fn test_cli_local_llm_flags() {
        let local_flag = Cli::try_parse_from(["cx", "--local"]).unwrap();
        assert_eq!(local_flag.local, Some(true));

        let local_config = Cli::try_parse_from([
            "cx",
            "--local",
            "--local-url", "http://127.0.0.1:11434/v1",
            "--local-model", "qwen2.5-coder:32b"
        ]).unwrap();
        assert_eq!(local_config.local, Some(true));
        assert_eq!(local_config.local_url.as_deref(), Some("http://127.0.0.1:11434/v1"));
        assert_eq!(local_config.local_model.as_deref(), Some("qwen2.5-coder:32b"));
    }

    #[test]
    fn test_cli_directory_override() {
        let short = Cli::try_parse_from(["cx", "-C", "/tmp/test-project"]).unwrap();
        assert_eq!(short.directory, Some(PathBuf::from("/tmp/test-project")));

        let long = Cli::try_parse_from(["cx", "--directory", "/home/user/code"]).unwrap();
        assert_eq!(long.directory, Some(PathBuf::from("/home/user/code")));
    }

    #[test]
    fn test_cli_fleet_flags() {
        let fleet = Cli::try_parse_from(["cx", "--fleet"]).unwrap();
        assert!(fleet.fleet);

        let sysadmin = Cli::try_parse_from(["cx", "--sysadmin"]).unwrap();
        assert!(sysadmin.fleet);
    }

    #[test]
    fn test_cli_positional_query() {
        let query = Cli::try_parse_from(["cx", "inspect", "src/lib.rs", "--verbose-details"]).unwrap();
        assert_eq!(query.query, vec!["inspect", "src/lib.rs", "--verbose-details"]);
    }

    #[test]
    fn test_cli_completions_flag() {
        let bash = Cli::try_parse_from(["cx", "--completions", "bash"]).unwrap();
        assert_eq!(bash.completions, Some(clap_complete::Shell::Bash));

        let zsh = Cli::try_parse_from(["cx", "--completions", "zsh"]).unwrap();
        assert_eq!(zsh.completions, Some(clap_complete::Shell::Zsh));
    }

    #[test]
    fn test_cli_upgrade_flags() {
        let upgrade = Cli::try_parse_from(["cx", "--upgrade"]).unwrap();
        assert!(upgrade.upgrade);

        assert!(Cli::try_parse_from(["cx", "--no-update-check"]).unwrap().no_update_check);
        assert!(!Cli::try_parse_from(["cx"]).unwrap().no_update_check);

        let update_alias = Cli::try_parse_from(["cx", "--update", "-f"]).unwrap();
        assert!(update_alias.upgrade);
        assert!(update_alias.force);
    }
}