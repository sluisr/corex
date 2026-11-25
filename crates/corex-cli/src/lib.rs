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

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OutputFormat {
    #[default]
    Text,
    Json,
    StreamJson,
}

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

    /// Automatically approve tool executions (YOLO normal mode)
    #[arg(short = 'y', long)]
    yolo: bool,

    /// Automatically approve all tool executions without restrictions (YOLO ALL mode)
    #[arg(long = "yolo-all")]
    yolo_all: bool,

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

    /// Include beta / pre-release versions when updating (only used if newer than stable)
    #[arg(long = "beta", alias = "pre")]
    beta: bool,

    /// Generate shell autocompletion script (bash, zsh, fish, powershell, elvish)
    #[arg(long = "completions", value_enum)]
    completions: Option<clap_complete::Shell>,

    /// Manage remote VPS servers and clusters (SysAdmin operations)
    #[arg(long = "fleet", alias = "sysadmin")]
    fleet: bool,

    /// Output format for headless mode (-p): text, json, or stream-json (NDJSON)
    #[arg(long = "output-format", value_enum, default_value_t = OutputFormat::Text)]
    output_format: OutputFormat,

    /// Maximum agentic tool-call turns in headless mode (-p) (default: 35)
    #[arg(long = "max-turns", default_value_t = 35)]
    max_turns: usize,
}

pub async fn run() -> Result<()> {
    let mut cli = Cli::parse();
    if cli.message.is_none() && cli.query.first().is_some_and(|word| word == "browser-host") {
        return corex_browser::run_native_host().await;
    }
    std::thread::spawn(|| {
        let _ = corex_browser::install_native_host();
        corex_core::file_lock::prune_stale_locks(std::time::Duration::from_secs(24 * 60 * 60));
    });
    if let Some(arg0) = std::env::args().next() {
        if (arg0.ends_with("cxs") || arg0.ends_with("cx-fleet")) && !cli.fleet {
            cli.fleet = true;
        }
    }
    if cli.no_update_check {
        corex_core::update::disable_update_check();
    }

    let explicit_directory = cli.directory.is_some();
    let mut workspace_dir = cli
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
                } else if s.model == "deepseek-v4-pro" || s.model == "deepseek-pro" || s.model == "deepseek-reasoner" {
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

        // Any `cx fleet <subcommand>` runs through the fleet CLI handler (which reports
        // "Unknown fleet subcommand" on typos instead of starting an LLM chat session).
        if cli.message.is_none() && !fleet_args.is_empty() {
            return fleet_cli::handle_fleet_cli(&workspace_dir, &fleet_args).await;
        }
    }

    let is_update_cmd = cli.upgrade
        || (cli.message.is_none()
            && !cli.query.is_empty()
            && (cli.query[0] == "update" || cli.query[0] == "upgrade" || cli.query[0] == "--update" || cli.query[0] == "--upgrade"));

    if is_update_cmd {
        let current = env!("CARGO_PKG_VERSION");
        let force = cli.force || cli.query.iter().any(|q| q == "--force" || q == "-f" || q == "force");
        let include_beta = cli.beta || cli.query.iter().any(|q| q == "--beta" || q == "--pre" || q == "beta" || q == "pre");
        println!("Corex In-Place Self-Updater (v{})", current);
        match corex_core::update::perform_self_update_with_channel(current, force, include_beta, |msg| {
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
                    if include_beta {
                        println!("To force re-download the latest beta/stable release, run:\n  cx update --beta --force\n");
                    } else {
                        println!("To force re-download the latest release, run:\n  cx update --force\n");
                    }
                }
            }
        }
        return Ok(());
    }

    let resume_result = cli.resume.as_deref().map(Session::load_by_id_or_tag);
    if !explicit_directory {
        let saved_dir = resume_result.as_ref().and_then(|r| r.as_ref().ok()).and_then(Session::existing_workspace);
        if let Some(dir) = saved_dir.filter(|dir| std::env::set_current_dir(dir).is_ok()) {
            workspace_dir = dir;
        }
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
            config.adopt_provider_for_model();
            if config.is_deepseek_endpoint() && !config.local_llm_enabled {
                if corex_core::config::is_pro_model(&config.model) {
                    config.reasoning_effort = config.pro_settings.reasoning_effort.clone();
                } else {
                    config.temperature = config.flash_settings.temperature;
                    config.reasoning_effort = config.flash_settings.reasoning_effort.clone();
                }
            }
            explicit_model = true;
        }
    }
    if let Some(u) = cli.base_url {
        if let Err(e) = corex_core::config::validate_base_url(&u) {
            anyhow::bail!("{}", e);
        }
        if cli.api_key.is_none()
            && !corex_core::config::is_deepseek_base_url(&u)
            && !corex_core::config::same_host(&u, &config.base_url)
        {
            config.api_key.clear();
        }
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
    if cli.yolo_all {
        config.yolo_mode = corex_core::config::YoloMode::All;
    } else if cli.yolo {
        config.yolo_mode = corex_core::config::YoloMode::Normal;
    }

    let mut resumed_session: Option<Session> = None;
    if let (Some(session_id), Some(result)) = (cli.resume.as_ref(), resume_result) {
        match result {
            Ok(mut loaded) => {
                if !explicit_model {
                    config.adopt_session_model(&loaded.model, loaded.provider.as_deref());
                    loaded.provider = config.active_provider.clone();
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
            .with_provider(config.active_provider.clone())
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
        run_headless(llm_client, workspace_dir, prompt, Some(display), config.yolo_mode, session, cli.output_format, cli.max_turns).await?;
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
        run_headless(llm_client, workspace_dir, prompt, Some(display), config.yolo_mode, session, cli.output_format, cli.max_turns).await?;
        return Ok(());
    }

    if headless_prompt.is_none() && !cli.query.is_empty() {
        if let Some(err_msg) = check_misclicked_cli_command(&cli.query) {
            anyhow::bail!("{}", err_msg);
        }
        headless_prompt = Some(cli.query.join(" "));
    }

    // Natural stdin pipe support: `git diff | cx -p "review this"` or `cat error.log | cx`.
    let piped_stdin = read_piped_stdin();
    let (headless_prompt, display_prompt) = combine_prompt_with_stdin(headless_prompt, piped_stdin);

    if let Some(prompt_text) = headless_prompt {
        let session = make_session(resumed_session);
        run_headless(llm_client, workspace_dir, prompt_text, display_prompt, config.yolo_mode, session, cli.output_format, cli.max_turns).await?;
        return Ok(());
    }

    let mut app = App::new(llm_client, workspace_dir.clone(), config.yolo_mode.is_active());
    app.set_yolo_mode(config.yolo_mode);
    app.start_mcp_background_load();
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

/// Detects mistyped CLI subcommands or TUI slash commands passed on the shell command line
/// without `-p`, returning a "Command not found" error instead of starting an LLM session.
fn check_misclicked_cli_command(query: &[String]) -> Option<String> {
    let first = query.first()?.trim();
    let lower = first.to_ascii_lowercase();

    // 1. Any single-segment `/command` typed in the shell (other than `/web <q>`, `/search <q>`, `/fetch <url>`
    // which were already handled above) is a TUI slash command or a typo of one.
    if lower.starts_with('/') && !lower[1..].contains('/') {
        return Some(format!(
            "Command not found: '{}'. Start 'cx' to use slash commands inside the TUI, or run 'cx --help' for CLI options.",
            first
        ));
    }

    // 2. Multi-word or single-word typo of a top-level CLI subcommand (`update`, `upgrade`, `fleet`,
    // `sysadmin`, `completions`, `browser-host`).
    const CLI_SUBCOMMANDS: &[&str] = &[
        "update",
        "upgrade",
        "fleet",
        "sysadmin",
        "completions",
        "browser-host",
    ];
    for &sub in CLI_SUBCOMMANDS {
        let dist = cli_edit_distance(&lower, sub);
        if dist > 0 && dist <= 2 {
            return Some(format!(
                "Command not found: '{}'. Did you mean 'cx {}'?",
                first, sub
            ));
        }
    }

    // 3. Single-token command name or typo (e.g. `cx model`, `cx stats`, `cx config`, `cx modle`, `cx status`).
    if query.len() == 1 {
        const TUI_AND_CLI_COMMANDS: &[(&str, &str)] = &[
            ("model", "cx -m <model> (or /model inside cx)"),
            ("models", "cx -m <model> (or /model inside cx)"),
            ("resume", "cx --resume <id|tag> (or /resume inside cx)"),
            ("sessions", "cx --list-sessions (or /chat list inside cx)"),
            ("chat", "cx --list-sessions (or /chat inside cx)"),
            ("web", "cx -w <query>"),
            ("search", "cx -w <query>"),
            ("fetch", "cx --fetch <url>"),
            ("local", "cx --local (or /local inside cx)"),
            ("yolo", "cx -y (or /yolo inside cx)"),
            ("help", "cx --help (or /help inside cx)"),
            ("version", "cx --version"),
            ("stats", "/stats inside cx"),
            ("context", "/context inside cx"),
            ("balance", "/balance inside cx"),
            ("wallet", "/wallet inside cx"),
            ("config", "/settings inside cx"),
            ("settings", "/settings inside cx"),
            ("key", "/key inside cx"),
            ("auth", "/auth inside cx"),
            ("login", "/key inside cx"),
            ("mcp", "/mcp inside cx"),
            ("tasks", "/tasks inside cx"),
            ("memory", "/memory inside cx"),
            ("plan", "/plan inside cx"),
            ("compact", "/compact inside cx"),
            ("compress", "/compress inside cx"),
            ("clear", "/clear inside cx"),
            ("export", "/export inside cx"),
            ("save", "/save inside cx"),
            ("new", "/new inside cx"),
            ("rewind", "/rewind inside cx"),
            ("info", "/info inside cx"),
            ("logs", "/logs inside cx"),
            ("fim", "/fim inside cx"),
            ("vps", "cx --fleet (or /fleet inside cx)"),
            ("status", "cx fleet status (or /stats inside cx)"),
            ("quit", "/quit inside cx"),
            ("exit", "/exit inside cx"),
        ];

        for &(cmd, hint) in TUI_AND_CLI_COMMANDS {
            let max_dist = if cmd.len() <= 4 { 1 } else { 2 };
            let dist = cli_edit_distance(&lower, cmd);
            if dist == 0 || dist <= max_dist {
                return Some(format!(
                    "Command not found: '{}'. Use {} — or pass '-p \"{}\"' to send it as a prompt.",
                    first, hint, first
                ));
            }
        }
    }

    None
}

fn cli_edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0; b_chars.len() + 1];
    for (i, &ca) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_chars.len()]
}

/// Puts the ask_user questions to whoever is on stdin and returns their answers. With nobody to
/// answer (stdin closed), says so plainly so the model goes on with a stated assumption instead
/// of waiting for a reply that never comes in a one-shot run.
fn ask_user_on_stdin(args: &serde_json::Value) -> String {
    let questions: Vec<(String, Vec<String>)> = match args.get("questions").and_then(|q| q.as_array()) {
        Some(list) => list
            .iter()
            .filter_map(|q| {
                let text = q.get("question")?.as_str()?.to_string();
                let options = q
                    .get("options")
                    .and_then(|o| o.as_array())
                    .map(|o| o.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                Some((text, options))
            })
            .collect(),
        None => args.get("question").and_then(|q| q.as_str()).map(|q| vec![(q.to_string(), Vec::new())]).unwrap_or_default(),
    };
    if questions.is_empty() {
        return "ask_user was called without any question.".to_string();
    }
    let mut answers = Vec::new();
    for (question, options) in &questions {
        println!("\n\x1b[1m?\x1b[0m {}", corex_core::sanitize_for_terminal(question));
        if !options.is_empty() {
            println!("  ({})", corex_core::sanitize_for_terminal(&options.join(" / ")));
        }
        print!("> ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(n) if n > 0 && !line.trim().is_empty() => answers.push(format!("- {}: {}", question, line.trim())),
            _ => {
                return "No user is available to answer (non-interactive run with no input). Do not wait for a reply: \
                        choose the most reasonable option, say which one you assumed, and continue."
                    .to_string()
            }
        }
    }
    format!("The user answered:\n{}", answers.join("\n"))
}

/// Reads piped stdin when stdin is not a TTY (capped at 256 KB to keep prompts safe).
fn read_piped_stdin() -> Option<String> {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return None;
    }
    const MAX_STDIN_BYTES: usize = 256 * 1024;
    let mut buf = Vec::new();
    let mut handle = std::io::stdin().take((MAX_STDIN_BYTES + 1) as u64);
    if handle.read_to_end(&mut buf).is_err() || buf.is_empty() {
        return None;
    }
    let truncated = buf.len() > MAX_STDIN_BYTES;
    if truncated {
        buf.truncate(MAX_STDIN_BYTES);
    }
    let text = String::from_utf8_lossy(&buf).trim().to_string();
    if text.is_empty() {
        return None;
    }
    if truncated {
        Some(format!("{text}\n\n[stdin truncated at 256 KB]"))
    } else {
        Some(text)
    }
}

fn combine_prompt_with_stdin(
    prompt: Option<String>,
    piped: Option<String>,
) -> (Option<String>, Option<String>) {
    match (prompt, piped) {
        (Some(p), Some(stdin_text)) => (
            Some(format!("{p}\n\n<stdin>\n{stdin_text}\n</stdin>")),
            Some(format!("{p} (+ piped stdin)")),
        ),
        (None, Some(stdin_text)) => (Some(stdin_text), Some("(piped stdin)".to_string())),
        (p, None) => (p, None),
    }
}

async fn run_headless(
    client: LlmClient,
    workspace_dir: PathBuf,
    user_prompt: String,
    display_prompt: Option<String>,
    yolo: corex_core::config::YoloMode,
    mut session: Session,
    output_format: OutputFormat,
    max_turns: usize,
) -> Result<()> {
    let mut tool_registry = ToolRegistry::new();
    corex_fleet::register_fleet_tools(&mut tool_registry);
    let mcp_cfg = client.get_config().mcp_servers;
    let has_enabled_mcp = mcp_cfg.values().any(|s| s.enabled);
    let has_sudo = get_sudo_password().is_some();
    let ws_for_prompt = workspace_dir.clone();
    let ws_for_fleet = workspace_dir.clone();

    let (mcp_loaded, (system_prompt_text, has_fleet_nodes)) = tokio::join!(
        async {
            if has_enabled_mcp {
                corex_tools::load_mcp_servers(&mcp_cfg).await.0
            } else {
                Vec::new()
            }
        },
        async move {
            tokio::task::spawn_blocking(move || {
                let prompt = PromptBuilder::new(&ws_for_prompt)
                    .with_sudo_password(has_sudo)
                    .build();
                let fleet_nodes = !corex_fleet::FleetConfig::load_fast(Some(&ws_for_fleet)).all_nodes().is_empty();
                (prompt, fleet_nodes)
            })
            .await
            .unwrap_or_default()
        }
    );
    if !mcp_loaded.is_empty() {
        tool_registry.register_many(mcp_loaded);
    }

    session.add_message(Message::user(user_prompt.clone()));

    let mut messages = Vec::with_capacity(session.messages.len() + 1);
    messages.push(Message::system(system_prompt_text));
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

    let structured = output_format != OutputFormat::Text;
    if !structured {
        use colored::Colorize;
        let you_badge = format!(
            "{}{}{}",
            "[".truecolor(95, 95, 95),
            "You".bold().truecolor(130, 215, 160),
            "]".truecolor(95, 95, 95),
        );
        println!("{} {}", you_badge, display_prompt.as_deref().unwrap_or(&user_prompt));
    }

    let mut tools = tool_registry.list_definitions();
    if !has_fleet_nodes {
        tools.retain(|d| d.function.name != "fleet");
    }
    let tool_registry = std::sync::Arc::new(tool_registry);
    let context = ToolContext {
        workspace_dir: workspace_dir.clone(),
        yolo_mode: yolo.is_active(),
        yolo_all: yolo.is_all(),
        sudo_password: get_sudo_password(),
        allowed_commands: client.get_config().allowed_commands.clone(),
        extra_dirs: Vec::new(),
    };

    use std::io::IsTerminal;
    let is_tty = !structured && std::io::stdout().is_terminal();
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
            let was_active = self.stop_tx.is_some() || self.handle.is_some();
            if let Some(tx) = self.stop_tx.take() {
                let _ = tx.send(());
            }
            if let Some(handle) = self.handle.take() {
                let _ = handle.await;
            }
            if was_active {
                print!("\r\x1b[2K");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
    }

    let max_headless_turns = max_turns.max(1);
    let mut turn_count = 0;
    let mut truncated_retries = 0u8;
    let mut final_response_text = String::new();
    let mut recorded_tool_calls: Vec<serde_json::Value> = Vec::new();

    loop {
        turn_count += 1;
        if turn_count > max_headless_turns {
            if !structured {
                eprintln!("\n[WARNING] Maximum headless tool iteration limit ({}) reached. Terminating loop to protect budget.", max_headless_turns);
            }
            break;
        }

        let cancel_token = CancellationToken::new();
        let mut request = messages.clone();
        corex_core::session_images::keep_recent_images(&mut request, client.get_config().recent_images, session.images_dir().as_deref());
        let mut active_spinner: Option<StatusSpinner> = Some(StatusSpinner::start(
            is_tty,
            "\x1b[90m",
            "Thinking...".to_string(),
        ));
        let mut rx = match client
            .stream_chat(request, Some(tools.clone()), cancel_token.clone())
            .await
        {
            Ok(rx) => rx,
            Err(e) => {
                if let Some(mut s) = active_spinner.take() {
                    s.stop().await;
                }
                return Err(e);
            }
        };

        let mut current_tool_calls: Vec<ToolCall> = Vec::new();
        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut current_extra_content: Option<serde_json::Value> = None;
        let mut current_stage: u8 = 1; // 0 = idle, 1 = reasoning/waiting, 2 = content
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
                    if output_format == OutputFormat::StreamJson {
                        println!("{}", serde_json::json!({ "type": "delta", "text": delta }));
                    }
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
                StreamEvent::Compacting => {
                    if let Some(mut s) = active_spinner.take() {
                        s.stop().await;
                    }
                    active_spinner = Some(StatusSpinner::start(
                        is_tty,
                        "\x1b[33m",
                        "Compacting context...".to_string(),
                    ));
                    current_stage = 0;
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
                    corex_tools::background::shutdown_all_tasks();
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
            final_response_text = assistant_text.clone();
            if output_format == OutputFormat::Text {
                if is_tty {
                    let theme = corex_tui::Theme::default();
                    let rendered = corex_tui::render_markdown_to_ansi(&safe_text, &theme, term_width);
                    println!("{}\n", rendered);
                } else {
                    println!("{}\n", safe_text);
                }
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

            struct HeadlessRunnable {
                idx: usize,
                call_id: String,
                tool_name: String,
                raw_args: String,
                args_json: serde_json::Value,
                action_label: &'static str,
                display_detail: String,
            }
            struct HeadlessDone {
                call_id: String,
                tool_name: String,
                raw_args: String,
                output: String,
                action_label: &'static str,
                display_detail: String,
                ok: bool,
                err_msg: Option<String>,
            }

            let mut ordered_results: Vec<Option<HeadlessDone>> =
                (0..current_tool_calls.len()).map(|_| None).collect();
            let mut to_run: Vec<HeadlessRunnable> = Vec::new();

            for (idx, call) in current_tool_calls.iter().enumerate() {
                let args_json = corex_core::parse_lenient_json(&call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);

                let (action_label, target_detail): (&str, std::borrow::Cow<'_, str>) = if call.function.name == "run_shell_command" || call.function.name == "shell" || call.function.name == "run_command" {
                    let cmd = args_json.get("command").and_then(|c| c.as_str()).unwrap_or("").trim();
                    ("Running command", std::borrow::Cow::Borrowed(cmd))
                } else if call.function.name == "read_file" {
                    let p = args_json.get("path").or_else(|| args_json.get("file_path")).and_then(|c| c.as_str()).unwrap_or("");
                    ("Reading file", std::borrow::Cow::Borrowed(p))
                } else if call.function.name == "write_file" {
                    let p = args_json.get("path").or_else(|| args_json.get("file_path")).and_then(|c| c.as_str()).unwrap_or("");
                    ("Writing file", std::borrow::Cow::Borrowed(p))
                } else if call.function.name == "edit" || call.function.name == "apply_patch" {
                    let p = args_json.get("path").or_else(|| args_json.get("file_path")).and_then(|c| c.as_str()).unwrap_or("");
                    ("Editing file", std::borrow::Cow::Borrowed(p))
                } else if call.function.name == "glob" || call.function.name == "ls" || call.function.name == "list_directory" {
                    let p = args_json.get("path").or_else(|| args_json.get("dir")).and_then(|c| c.as_str()).unwrap_or(".");
                    ("Exploring directory", std::borrow::Cow::Borrowed(p))
                } else if call.function.name == "web_search" || call.function.name == "google_web_search" {
                    let mut queries: Vec<&str> = Vec::new();
                    if let Some(q) = args_json.get("query").and_then(|c| c.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                        queries.push(q);
                    }
                    if let Some(list) = args_json.get("queries").and_then(|v| v.as_array()) {
                        for q in list.iter().filter_map(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                            if !queries.contains(&q) {
                                queries.push(q);
                            }
                        }
                    }
                    let joined = queries.join(" · ");
                    ("Searching web", std::borrow::Cow::Owned(joined))
                } else if call.function.name == "web_fetch" {
                    let u = args_json.get("url").and_then(|c| c.as_str()).unwrap_or("");
                    ("Fetching web page", std::borrow::Cow::Borrowed(u))
                } else {
                    ("Running tool", std::borrow::Cow::Borrowed(call.function.name.as_str()))
                };

                let display_detail = corex_core::truncate_ellipsis(&corex_core::sanitize_for_terminal(&target_detail), 70);

                if let Some(tool) = tool_registry.get(&call.function.name) {
                    let must_ask = corex_tools::policy::verdict(&tool_registry, &call.function.name, &args_json, &context, false)
                        == corex_tools::policy::Verdict::Ask;
                    if must_ask {
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
                            ordered_results[idx] = Some(HeadlessDone {
                                call_id: call.id.clone(),
                                tool_name: call.function.name.clone(),
                                raw_args: call.function.arguments.clone(),
                                output: "Execution denied by user.".to_string(),
                                action_label,
                                display_detail,
                                ok: true,
                                err_msg: None,
                            });
                            continue;
                        }
                    }
                }

                if call.function.name == "ask_user" {
                    let answer = ask_user_on_stdin(&args_json);
                    ordered_results[idx] = Some(HeadlessDone {
                        call_id: call.id.clone(),
                        tool_name: call.function.name.clone(),
                        raw_args: call.function.arguments.clone(),
                        output: answer,
                        action_label,
                        display_detail,
                        ok: true,
                        err_msg: None,
                    });
                    continue;
                }

                to_run.push(HeadlessRunnable {
                    idx,
                    call_id: call.id.clone(),
                    tool_name: call.function.name.clone(),
                    raw_args: call.function.arguments.clone(),
                    args_json,
                    action_label,
                    display_detail,
                });
            }

            if !to_run.is_empty() {
                let spinner_msg = if to_run.len() == 1 {
                    format!("\x1b[1m{}:\x1b[0m \x1b[36m{}\x1b[0m", to_run[0].action_label, to_run[0].display_detail)
                } else {
                    format!("\x1b[1mRunning {} tools in parallel...\x1b[0m", to_run.len())
                };
                let mut tool_spinner = StatusSpinner::start(is_tty, "\x1b[33m", spinner_msg);

                let mut join_set = tokio::task::JoinSet::new();
                for item in to_run {
                    let reg = tool_registry.clone();
                    let ctx = context.clone();
                    join_set.spawn(async move {
                        let res = reg.execute(&item.tool_name, item.args_json, &ctx).await;
                        (item.idx, item.call_id, item.tool_name, item.raw_args, item.action_label, item.display_detail, res)
                    });
                }

                while let Some(joined) = join_set.join_next().await {
                    if let Ok((idx, call_id, tool_name, raw_args, action_label, display_detail, res)) = joined {
                        let done = match res {
                            Ok(o) => HeadlessDone {
                                call_id,
                                tool_name,
                                raw_args,
                                output: o.output,
                                action_label,
                                display_detail,
                                ok: true,
                                err_msg: None,
                            },
                            Err(e) => {
                                let msg = format!("Error executing {}: {}", tool_name, e);
                                HeadlessDone {
                                    call_id,
                                    tool_name,
                                    raw_args,
                                    output: msg,
                                    action_label,
                                    display_detail,
                                    ok: false,
                                    err_msg: Some(e.to_string()),
                                }
                            }
                        };
                        ordered_results[idx] = Some(done);
                    }
                }
                tool_spinner.stop().await;
            }

            for done in ordered_results.into_iter().flatten() {
                if structured {
                    let ev = serde_json::json!({
                        "type": "tool_done",
                        "name": done.tool_name,
                        "ok": done.ok,
                        "detail": done.display_detail,
                    });
                    if output_format == OutputFormat::StreamJson {
                        println!("{ev}");
                    }
                    recorded_tool_calls.push(ev);
                }
                if is_tty && done.tool_name != "ask_user" && done.output != "Execution denied by user." {
                    if done.ok {
                        println!("\x1b[32m✓\x1b[0m \x1b[1m{}\x1b[0m: \x1b[90m{}\x1b[0m", done.action_label, done.display_detail);
                    } else {
                        println!(
                            "\x1b[31m✗\x1b[0m \x1b[1m{}\x1b[0m: \x1b[90m{}\x1b[0m ({})",
                            done.action_label,
                            done.display_detail,
                            done.err_msg.as_deref().unwrap_or("error")
                        );
                    }
                }
                if let Some(ref e) = done.err_msg {
                    eprintln!("Error executing {}: {}", done.tool_name, e);
                }
                if done.tool_name == "ask_user" || done.output == "Execution denied by user." {
                    let msg = Message::tool_response(done.call_id, done.output);
                    messages.push(msg.clone());
                    session.add_message(msg);
                    continue;
                }
                // Huge output is cut to head + tail; the full text stays readable from a file.
                let output = corex_core::spill_large_output(&done.tool_name, done.output);
                // Advisory nudge when the model keeps repeating the exact same call.
                let output = match corex_core::observe_tool_call(&done.tool_name, &done.raw_args) {
                    Some(reminder) => format!("{}\n\n{}", output, reminder),
                    None => output,
                };
                let tool_msg = Message::tool_response(done.call_id, output);
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
            let mut final_msg = Message::assistant(assistant_text, cot_opt);
            final_msg.extra_content = current_extra_content.take();
            session.add_message(final_msg);
        }
        break;
    }

    corex_tools::background::shutdown_all_tasks();
    let _ = session.save();
    if structured {
        let cfg = client.get_config();
        let payload = serde_json::json!({
            "type": "done",
            "response": final_response_text,
            "model": cfg.model,
            "provider": cfg.active_provider,
            "turns": turn_count.min(max_headless_turns),
            "tool_calls": recorded_tool_calls,
            "usage": {
                "prompt_tokens": session.total_usage.prompt_tokens,
                "completion_tokens": session.total_usage.completion_tokens,
                "total_tokens": session.total_usage.total_tokens,
            }
        });
        println!("{payload}");
    }
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
    fn test_cli_output_format_max_turns_and_stdin_combine() {
        let cli = Cli::try_parse_from([
            "cx",
            "-p",
            "review",
            "--output-format",
            "json",
            "--max-turns",
            "5",
        ])
        .unwrap();
        assert_eq!(cli.output_format, OutputFormat::Json);
        assert_eq!(cli.max_turns, 5);

        let stream_cli = Cli::try_parse_from(["cx", "-p", "hi", "--output-format", "stream-json"]).unwrap();
        assert_eq!(stream_cli.output_format, OutputFormat::StreamJson);

        let (combined, display) = combine_prompt_with_stdin(
            Some("review this".to_string()),
            Some("diff --git a/x b/x".to_string()),
        );
        assert!(combined.unwrap().contains("<stdin>\ndiff --git a/x b/x\n</stdin>"));
        assert_eq!(display.as_deref(), Some("review this (+ piped stdin)"));

        let (stdin_only, display_only) = combine_prompt_with_stdin(None, Some("error log".to_string()));
        assert_eq!(stdin_only.as_deref(), Some("error log"));
        assert_eq!(display_only.as_deref(), Some("(piped stdin)"));
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
        assert!(!short.yolo_all);

        let long = Cli::try_parse_from(["cx", "--yolo"]).unwrap();
        assert!(long.yolo);
        assert!(!long.yolo_all);

        let all = Cli::try_parse_from(["cx", "--yolo-all"]).unwrap();
        assert!(all.yolo_all);
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

    #[test]
    fn test_misclicked_cli_commands_are_rejected_without_starting_session() {
        assert!(
            check_misclicked_cli_command(&["updte".to_string()])
                .is_some_and(|m| m.contains("Command not found: 'updte'") && m.contains("cx update"))
        );
        assert!(
            check_misclicked_cli_command(&["flet".to_string(), "status".to_string()])
                .is_some_and(|m| m.contains("Command not found: 'flet'") && m.contains("cx fleet"))
        );
        assert!(
            check_misclicked_cli_command(&["/modle".to_string()])
                .is_some_and(|m| m.contains("Command not found: '/modle'"))
        );
        assert!(
            check_misclicked_cli_command(&["model".to_string()])
                .is_some_and(|m| m.contains("Command not found: 'model'"))
        );
        assert!(
            check_misclicked_cli_command(&["modle".to_string()])
                .is_some_and(|m| m.contains("Command not found: 'modle'"))
        );
        // Normal multi-word prompts still pass through
        assert!(check_misclicked_cli_command(&["inspect".to_string(), "src/lib.rs".to_string()]).is_none());
    }
}