//! The slash commands typed in the prompt (`/plan`, `/memory`, `/directory`, ...).

use super::*;

impl App {
    pub async fn handle_slash_command(&mut self, cmd: &str) -> bool {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        let name = parts.first().copied().unwrap_or("");

        match name {
            "/clear" => {
                self.session.clear_messages();
                self.invalidate_message_cache();
                self.clear_selection();
                self.thinking_state.reset();
                self.streaming_text.clear();
                true
            }
            "/plan" => {
                self.plan_mode = !self.plan_mode;
                let msg = if self.plan_mode {
                    "Plan mode on: Corex reads and plans. Edits, memory, subagents and MCP are off, and shell commands that are not read-only wait for you."
                } else {
                    "Plan mode off."
                };
                self.session.add_message(Message::system(msg));
                true
            }
            "/directory" | "/dir" => {
                let msg = match parts.get(1).copied().unwrap_or("show") {
                    "show" => self.directory_report(),
                    "add" if parts.len() > 2 => parts[2..]
                        .join(" ")
                        .split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(|p| match self.add_directory(p) {
                            Ok(dir) => format!("Added {}: the agent can read it without confirmation.", dir.display()),
                            Err(e) => format!("Could not add {}: {}", p, e),
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => "Usage: /directory add <path>[,<path>...] | show".to_string(),
                };
                self.session.add_message(Message::system(msg));
                true
            }
            "/memory" => {
                let memory = Memory::new(&self.workspace_dir);
                let msg = match parts.get(1).copied().unwrap_or("list") {
                    "list" => memory_file_list(&memory),
                    "show" => {
                        let memory = PromptBuilder::new(&self.workspace_dir).memory_prompt();
                        if memory.is_empty() {
                            "Memory is empty: no instruction files found.".to_string()
                        } else {
                            format!("Memory block sent to the model:\n{}", memory.trim())
                        }
                    }
                    "add" if parts.len() > 2 => {
                        let (scope, words) = match parts[2] {
                            "--project" => (MemoryScope::Project, &parts[3..]),
                            _ => (MemoryScope::Global, &parts[2..]),
                        };
                        if words.is_empty() {
                            "Usage: /memory add [--project] <text>".to_string()
                        } else {
                            match memory.add(scope, &words.join(" ")) {
                                Ok(target) => format!("Added to {}, applies from the next message.", target),
                                Err(e) => format!("Could not add to memory: {}", e),
                            }
                        }
                    }
                    "reload" | "refresh" => {
                        PromptBuilder::new(&self.workspace_dir).invalidate_cache();
                        format!("Memory reloaded, applies from the next message.\n\n{}", memory_file_list(&memory))
                    }
                    _ => "Usage: /memory list | show | add [--project] <text> | reload".to_string(),
                };
                self.session.add_message(Message::system(msg));
                true
            }
            "/stats" | "/context" => {
                let u = &self.session.total_usage;
                let ratio = u.cache_hit_percentage();
                let cfg = self.llm_client.get_config();
                let mut msg = format!(
                    "Session Usage Metrics:\n- Prompt Tokens: {}\n- Cached Prompt Tokens: {} ({:.1}% KV Cache Hit)\n- Completion Tokens: {}\n- Total Tokens: {}",
                    u.prompt_tokens, u.prompt_cache_hit_tokens, ratio, u.completion_tokens, u.total_tokens
                );
                let used = self.session.context_tokens();
                let (window, window_source) = if cfg.model.starts_with("local") {
                    (24_000, corex_core::WindowSource::Default)
                } else {
                    cfg.context_window_with_source()
                };
                msg.push_str(&format!(
                    "\n\nContext:\n- In use: ~{} of {} tokens ({:.1}%)\n- Window: {}\n- Auto-compact: {} at {} tokens\n- History cap (oldest turns dropped above): {} tokens\n- Change with `context_window_tokens`, `compact_threshold_tokens` and `history_token_limit` in ~/.corex/settings.json",
                    group_thousands(used as u64),
                    group_thousands(window as u64),
                    used as f64 * 100.0 / window.max(1) as f64,
                    window_source.label(),
                    if cfg.auto_compact { "on" } else { "off" },
                    group_thousands(cfg.effective_compact_threshold() as u64),
                    group_thousands(cfg.effective_history_limit() as u64),
                ));
                if cfg.is_deepseek_endpoint() && !cfg.local_llm_enabled && u.total_tokens > 0 {
                    let is_pro = cfg.model.contains("pro") || cfg.model.contains("reasoner");
                    let (cached_rate, miss_rate, comp_rate) = if is_pro {
                        (0.14 / 1_000_000.0, 0.55 / 1_000_000.0, 2.19 / 1_000_000.0)
                    } else {
                        (0.014 / 1_000_000.0, 0.14 / 1_000_000.0, 0.28 / 1_000_000.0)
                    };
                    let miss = u.prompt_tokens.saturating_sub(u.prompt_cache_hit_tokens);
                    let actual_cost = (u.prompt_cache_hit_tokens as f64 * cached_rate)
                        + (miss as f64 * miss_rate)
                        + (u.completion_tokens as f64 * comp_rate);
                    let un_cached = (u.prompt_tokens as f64 * miss_rate) + (u.completion_tokens as f64 * comp_rate);
                    let saved = (un_cached - actual_cost).max(0.0);
                    msg.push_str(&format!("\n- Estimated DeepSeek Cost: ${:.6} USD (Saved ${:.6} with KV Cache)", actual_cost, saved));
                }
                self.session.add_message(Message::system(msg));
                true
            }
            "/local" => {
                if parts.len() > 1 && parts[1] == "status" {
                    let cfg = self.llm_client.get_config();
                    let local = self.llm_client.local_client();
                    let is_up = local.health_check().await;
                    let msg = format!(
                        "Local LLM Status:\n- Enabled: {}\n- Endpoint: {}\n- Model: {}\n- Server Reachable: {}",
                        cfg.local_llm_enabled,
                        cfg.local_llm_url,
                        cfg.local_llm_model,
                        if is_up { "YES (Active)" } else { "NO (Offline)" }
                    );
                    self.session.add_message(Message::system(msg));
                    return true;
                }

                if parts.len() > 1 {
                    let prompt = parts[1..].join(" ");
                    let local = self.llm_client.local_client();
                    self.session.add_message(Message::user(format!("/local {}", prompt)));
                    match local.quick_chat(&prompt).await {
                        Ok(answer) => {
                            self.session.add_message(Message::assistant(
                                format!("{}\n\n*(Answered by Local LLM @ $0.00)*", answer.trim()),
                                None,
                            ));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!(
                                "Local LLM Error (is llama-server running on {}?): {}",
                                self.llm_client.get_config().local_llm_url,
                                e
                            )));
                        }
                    }
                } else {
                    self.session.add_message(Message::system("Usage: /local <prompt> (ask local LLM directly) or /local status"));
                }
                true
            }
            "/web" | "/search" => {
                if parts.len() > 1 {
                    // Handled by streaming chat loop so the AI invokes web_search and synthesizes the answer
                    false
                } else {
                    self.session.add_message(Message::system("Usage: /web <query> (Search the web with DeepSeek and synthesize the answer)"));
                    true
                }
            }
            "/fetch" => {
                if parts.len() > 1 {
                    // Handled by streaming chat loop so the AI invokes web_fetch and summarizes the page
                    false
                } else {
                    self.session.add_message(Message::system("Usage: /fetch <url> (Fetch a webpage into Markdown using DeepSeek Fetch)"));
                    true
                }
            }

            "/balance" | "/wallet" => {
                if self.is_checking_balance {
                    self.set_status("Checking account balance...");
                    return true;
                }
                let msg_idx = self.session.messages.len();
                self.session.add_message(Message::system("Checking account balance..."));
                self.pending_balance_msg_index = Some(msg_idx);
                self.invalidate_message_cache();
                self.set_status("Checking account balance...");
                self.is_checking_balance = true;

                let client = self.llm_client.clone();
                let tx = self.balance_tx.clone();
                tokio::spawn(async move {
                    let res = client.check_balance().await.map_err(|e| e.to_string());
                    let _ = tx.send(res).await;
                });
                true
            }
            "/chat" | "/sessions" => {
                let is_fleet = self.fleet_dialog.is_open || self.normal_session_stash.is_some() || self.session.is_fleet();
                let subcmd = parts.get(1).copied().unwrap_or("list");
                match subcmd {
                    "list" => {
                        self.session_dialog.open(&self.workspace_dir.display().to_string(), is_fleet);
                    }
                    "save" => {
                        if parts.len() > 2 {
                            let tag = parts[2..].join(" ");
                            if let Err(e) = self.session.save_checkpoint(&tag) {
                                self.session.add_message(Message::system(format!("Failed to save checkpoint: {}", e)));
                            } else {
                                self.session.add_message(Message::system(format!("Conversation checkpoint saved with tag: {}.", tag)));
                            }
                        } else {
                            self.session.add_message(Message::system("Missing tag. Usage: /chat save <tag> or /save <tag>"));
                        }
                    }
                    "resume" | "load" => {
                        if parts.len() > 2 {
                            let target = parts[2..].join(" ");
                            match Session::load_by_id_or_tag(&target) {
                                Ok(loaded) => {
                                    let mut cfg = self.llm_client.get_config();
                                    cfg.model = loaded.model.clone();
                                    if let Some(t) = loaded.temperature {
                                        cfg.temperature = t;
                                    }
                                    if let Some(ref r) = loaded.reasoning_effort {
                                        cfg.reasoning_effort = r.clone();
                                    }
                                    self.llm_client.update_config(cfg);
                                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                                    let title = loaded.title.clone();
                                    let model = loaded.model.clone();
                                    self.session = loaded;
                                    self.cached_session_id.clear();
                                    self.invalidate_message_cache();
                                    self.clear_selection();
                                    self.scroll_to_bottom();
                                    self.session.add_message(Message::system(format!("Resumed session '{}' (Model: {}).", title, model)));
                                }
                                Err(e) => {
                                    self.session.add_message(Message::system(format!("Error: {}", e)));
                                }
                            }
                        } else {
                            self.session.add_message(Message::system("Missing session tag or number. Usage: /chat resume <tag/id>"));
                        }
                    }
                    "delete" | "rm" => {
                        if parts.len() > 2 {
                            let target = parts[2..].join(" ");
                            match Session::delete_by_id_or_tag(&target) {
                                Ok(id) => {
                                    self.session.add_message(Message::system(format!("Deleted session {}.", id)));
                                }
                                Err(e) => {
                                    self.session.add_message(Message::system(format!("Error deleting session: {}", e)));
                                }
                            }
                        } else {
                            self.session.add_message(Message::system("Missing session tag or number. Usage: /chat delete <tag/id>"));
                        }
                    }
                    "new" => {
                        let _ = self.session.save();
                        let cfg = self.llm_client.get_config();
                        self.session = Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&self.workspace_dir));
                        self.streaming_text.clear();
                        self.thinking_state.reset();
                        self.cached_session_id.clear();
                        self.invalidate_message_cache();
                        self.clear_selection();
                        self.scroll_to_bottom();
                        corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                        self.session.add_message(Message::system(format!("Started new chat session (Model: {}).", cfg.model)));
                    }
                    _ => {
                        self.session.add_message(Message::system("Usage: /chat list | /chat save <tag> | /chat resume <tag/id> | /chat delete <tag/id> | /chat new"));
                    }
                }
                true
            }
            "/resume" => {
                let is_fleet = self.fleet_dialog.is_open || self.normal_session_stash.is_some() || self.session.is_fleet();
                if parts.len() > 1 {
                    let target = parts[1..].join(" ");
                    match Session::load_by_id_or_tag(&target) {
                        Ok(mut loaded) => {
                            let mut cfg = self.llm_client.get_config();
                            cfg.model = loaded.model.clone();
                            if let Some(t) = loaded.temperature {
                                cfg.temperature = t;
                            }
                            if let Some(ref r) = loaded.reasoning_effort {
                                cfg.reasoning_effort = r.clone();
                            }
                            self.llm_client.update_config(cfg);
                            corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                            let title = loaded.title.clone();
                            let model = loaded.model.clone();
                            if is_fleet {
                                loaded.tag = Some("fleet".to_string());
                            }
                            self.session = loaded;
                            self.cached_session_id.clear();
                            self.invalidate_message_cache();
                            self.clear_selection();
                            self.scroll_to_bottom();
                            let label = if is_fleet { "Fleet session" } else { "session" };
                            self.session.add_message(Message::system(format!("Resumed {} '{}' (Model: {}).", label, title, model)));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!("Error: {}", e)));
                        }
                    }
                } else {
                    self.slash_popup_height_current = 0.0;
                    self.session_dialog.open(&self.workspace_dir.display().to_string(), is_fleet);
                }
                true
            }
            "/save" => {
                if parts.len() > 1 {
                    let tag = parts[1..].join(" ");
                    if let Err(e) = self.session.save_checkpoint(&tag) {
                        self.session.add_message(Message::system(format!("Failed to save checkpoint: {}", e)));
                    } else {
                        self.session.add_message(Message::system(format!("Conversation checkpoint saved with tag: {}.", tag)));
                    }
                } else {
                    self.session.add_message(Message::system("Missing tag. Usage: /save <tag>"));
                }
                true
            }
            "/new" => {
                let _ = self.session.save();
                let cfg = self.llm_client.get_config();
                self.session = Session::new_with_params(&cfg.model, cfg.temperature, &cfg.reasoning_effort, Some(&self.workspace_dir));
                self.streaming_text.clear();
                self.thinking_state.reset();
                self.cached_session_id.clear();
                self.invalidate_message_cache();
                self.clear_selection();
                self.scroll_to_bottom();
                corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                self.session.add_message(Message::system(format!("Started new chat session (Model: {}).", cfg.model)));
                true
            }
            "/model" => {
                if parts.len() > 1 {
                    let new_model = parts[1];
                    let mut cfg = self.llm_client.get_config();
                    cfg.model = new_model.to_string();
                    if !new_model.contains("local") {
                        cfg.local_llm_enabled = false;
                        // A model outside the catalog leaves the provider to be inferred from its name.
                        cfg.active_provider = cfg
                            .all_providers()
                            .into_iter()
                            .find(|p| !p.is_local() && p.models.iter().any(|m| m.id == new_model))
                            .map(|p| p.name);
                    }
                    let _ = cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.llm_client.update_config(cfg.clone());
                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                    self.session.model = new_model.to_string();
                    self.session.temperature = Some(cfg.temperature);
                    self.session.reasoning_effort = Some(cfg.reasoning_effort);
                    let _ = self.session.save();
                    self.session.add_message(Message::system(format!("Switched and saved active model to '{}'.", new_model)));
                } else {
                    let cfg = self.llm_client.get_config();
                    self.slash_popup_height_current = 0.0;
                    self.model_dialog.open(&cfg);
                }
                true
            }
            "/sudo" => {
                if parts.len() > 1 && !(parts.len() == 2 && parts[1] == "clear") {
                    let pwd = parts[1..].join(" ");
                    set_sudo_password(Some(pwd));
                    self.session.add_message(Message::system("Sudo password stored in session RAM (silent AskPass enabled)."));
                } else {
                    set_sudo_password(None);
                    self.session.add_message(Message::system("Sudo password cleared from session RAM."));
                }
                true
            }
            "/key" | "/auth" => {
                let cfg = self.llm_client.get_config();
                let prov = cfg.active_provider.clone().unwrap_or_else(|| "deepseek".to_string());
                let p_opt = cfg.all_providers().into_iter().find(|p| p.name == prov);
                let label = p_opt.as_ref().map(|p| p.display_name()).unwrap_or("DeepSeek").to_string();
                let env_var = p_opt.as_ref().map(|p| p.api_key_env.as_str()).unwrap_or("DEEPSEEK_API_KEY").to_string();

                if parts.len() > 1 {
                    let key_str = parts[1..].join(" ").trim().to_string();
                    let mut new_cfg = cfg.clone();
                    if prov == "deepseek" {
                        new_cfg.api_key = key_str.clone();
                    } else {
                        new_cfg.provider_api_keys.insert(prov.clone(), key_str.clone());
                    }
                    let _ = new_cfg.save_with_workspace(Some(&self.workspace_dir));
                    self.llm_client.update_config(new_cfg);
                    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(self.llm_client.clone()));
                    if let Some(pos) = self.model_dialog.providers.iter().position(|p| p.name == prov) {
                        if pos < self.model_dialog.ready.len() {
                            self.model_dialog.ready[pos] = true;
                        }
                    }
                    let note = crate::auth_dialog::env_override_note(&prov, &key_str).unwrap_or_default();
                    self.session.add_message(Message::system(format!(
                        "API key updated and saved for {} (${})!{}",
                        label, env_var, note
                    )));
                } else {
                    let portal = self.model_dialog.providers.iter()
                        .find(|p| p.name == prov)
                        .map(|p| p.portal_url())
                        .unwrap_or("https://platform.deepseek.com");
                    self.auth_dialog.open_for_provider(None, &prov, &label, &env_var, portal);
                }
                true
            }
            "/rewind" => {
                if self.session.user_turns().is_empty() {
                    self.session.add_message(Message::system("Nothing to rewind: no messages yet."));
                } else {
                    self.rewind_dialog.open(&self.session);
                }
                true
            }
            "/compact" | "/compress" => {
                let before = self.session.messages.clone();
                match self.llm_client.compact_messages(&mut self.session.messages, true).await {
                    Ok(Some(notice)) => {
                        self.session.keep_history_before_compaction(&before);
                        self.invalidate_message_cache();
                        let _ = self.session.save();
                        self.session.add_message(Message::system(notice));
                    }
                    Ok(None) => {
                        self.session.add_message(Message::system(
                            "Conversation history is too short to require compaction."
                        ));
                    }
                    Err(e) => {
                        self.session.add_message(Message::system(format!(
                            "Failed to compact conversation: {}", e
                        )));
                    }
                }
                true
            }
            "/info" | "/author" | "/credits" | "/about" => {
                let cfg = self.llm_client.get_config();
                let info = format!(
                    "COREX_INFO_CARD|{version}|{model}|{base_url}|{local_engine}|{local_enabled}|{session_id}|{workspace}|{branch}",
                    version = env!("CARGO_PKG_VERSION"),
                    model = cfg.model,
                    base_url = cfg.base_url,
                    local_engine = cfg.local_llm_url,
                    local_enabled = cfg.local_llm_enabled,
                    session_id = self.session.id,
                    workspace = self.workspace_dir.display(),
                    branch = self.git_branch
                );
                self.session.add_message(Message::system(info));
                true
            }
            "/update" => {
                let current = env!("CARGO_PKG_VERSION");
                let apply_update = parts.get(1).map(|s| *s == "apply" || *s == "--apply" || *s == "force" || *s == "--force").unwrap_or(false);
                let force = parts.get(1).map(|s| *s == "force" || *s == "--force").unwrap_or(false);

                if apply_update {
                    self.session.add_message(Message::system("Checking release and applying in-place update..."));
                    match corex_core::update::perform_self_update(current, force, |_| {}).await {
                        Ok(summary) => {
                            self.session.add_message(Message::system(format!(
                                "✓ Corex successfully updated to v{} (previous: v{})!\nBinary: {}\n\nPlease restart Corex to run the updated version.",
                                summary.updated_version, summary.previous_version, summary.binary_path.display()
                            )));
                        }
                        Err(e) => {
                            self.session.add_message(Message::system(format!(
                                "✕ In-place update: {}\n\nManual update alternatives:\n• cx update (in terminal)\n• npm install -g @sluisr/corex\n• cargo install --git https://github.com/sluisr/corex.git --force",
                                e
                            )));
                        }
                    }
                    return true;
                }

                let maybe_newer = self.update_available.lock().ok().and_then(|l| l.clone());
                let msg = if let Some(newer) = maybe_newer {
                    format!(
                        "A new version of Corex is available: v{} → v{}\n\n\
                        To update automatically in-place, run:\n\
                        • /update apply (or cx update in your terminal)\n\n\
                        Manual alternatives:\n\
                        • Via npm:       npm install -g @sluisr/corex\n\
                        • From source:   cargo install --git https://github.com/sluisr/corex.git --force\n\
                        • Releases:      https://github.com/sluisr/corex/releases/latest",
                        current, newer
                    )
                } else {
                    format!(
                        "Corex is up to date (v{}).\n\n\
                        To force re-download the latest release in-place:\n\
                        • /update force\n\n\
                        Manual update alternatives:\n\
                        • npm install -g @sluisr/corex\n\
                        • cargo install --git https://github.com/sluisr/corex.git --force\n\
                        • https://github.com/sluisr/corex/releases",
                        current
                    )
                };
                self.session.add_message(Message::system(msg));
                true
            }
            "/mouse" => {
                self.toggle_mouse_capture();
                true
            }
            "/keyboard" => {
                self.tap_keyboard.request_keyboard(Instant::now(), &mut self.mouse_capture);
                self.session.add_message(Message::system("Tap the screen to raise the keyboard: mouse reporting is paused for a few seconds so Termux handles the tap."));
                true
            }
            "/prefix" => {
                if parts.len() > 1 {
                    let prefix_text = parts[1..].join(" ");
                    let trimmed = prefix_text.trim();
                    if matches!(trimmed.to_ascii_lowercase().as_str(), "off" | "clear" | "none") {
                        self.response_prefix = None;
                        self.session.add_message(Message::system("Response formatting prefix cleared."));
                    } else {
                        self.response_prefix = Some(prefix_text.clone());
                        self.session.add_message(Message::system(format!(
                            "Response formatting prefix configured: '{}'. DeepSeek will direct its next output with this prefix.",
                            prefix_text
                        )));
                    }
                } else {
                    self.session.add_message(Message::system("Usage: /prefix <text> (e.g. /prefix Output: or /prefix ```json, or /prefix off to clear)"));
                }
                true
            }
            "/fim" => {
                if parts.len() > 1 {
                    let file_arg = parts[1];
                    let file_path = if file_arg.starts_with('/') {
                        PathBuf::from(file_arg)
                    } else {
                        self.workspace_dir.join(file_arg)
                    };

                    if !corex_tools::fs_tools::is_within_workspace(&file_path, &self.workspace_dir)
                        || corex_tools::fs_tools::is_sensitive_system_path(&file_path)
                    {
                        self.session.add_message(Message::system(format!(
                            "Refused to load '{}': path is outside the active workspace or points to a sensitive file.",
                            file_arg
                        )));
                        return true;
                    }

                    const FIM_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
                    const FIM_WINDOW_BYTES: usize = 24_000;

                    let file_size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                    if !file_path.exists() {
                        self.session.add_message(Message::system(format!("File '{}' not found.", file_arg)));
                    } else if file_size > FIM_MAX_FILE_BYTES {
                        self.session.add_message(Message::system(format!(
                            "File '{}' is too large for FIM ({} bytes, max {}).",
                            file_arg, file_size, FIM_MAX_FILE_BYTES
                        )));
                    } else {
                        match std::fs::read_to_string(&file_path) {
                            Ok(content) => {
                                if let Some(hole_idx) = content.find("<FIM_HOLE>") {
                                    let snippet = fim_context_window(&content, hole_idx, FIM_WINDOW_BYTES);
                                    let prompt = format!(
                                        "Please perform Fill-in-the-Middle (FIM) code completion for `{}`. Complete the exact code that belongs strictly inside `<FIM_HOLE>`:\n\n```\n{}\n```",
                                        file_arg,
                                        snippet
                                    );
                                    self.session.add_message(Message::system("FIM prompt loaded. Streaming completion..."));
                                    corex_core::reset_repeat_guard();
                                    self.session.add_message(Message::user(prompt));
                                } else {
                                    self.session.add_message(Message::system(format!(
                                        "FIM for '{}': Insert `<FIM_HOLE>` at the exact position in `{}` where code should be filled, then run `/fim {}` again.",
                                        file_arg, file_arg, file_arg
                                    )));
                                }
                            }
                            Err(e) => {
                                self.session.add_message(Message::system(format!("Error reading {}: {}", file_arg, e)));
                            }
                        }
                    }
                } else {
                    self.session.add_message(Message::system("Usage: /fim <path/to/file> (Fill-in-the-Middle code completion using <FIM_HOLE>)"));
                }
                true
            }
            "/mcp" => {
                let sub = parts.get(1).map(|s| s.to_lowercase()).unwrap_or_else(|| "status".to_string());
                match sub.as_str() {
                    "catalog" | "available" => {
                        let catalog = corex_tools::get_mcp_catalog();
                        let mut msg = format!("MCP Server Catalog ({} vetted servers):\n\n", catalog.len());
                        for item in catalog {
                            msg.push_str(&format!("• **{}** (`{}`)\n", item.name, item.id));
                            msg.push_str(&format!("  *{}*\n", item.description));
                            msg.push_str(&format!("  Command: `{} {}`\n", item.command, item.default_args.join(" ")));
                            if !item.env_keys.is_empty() {
                                msg.push_str(&format!("  Required Env: `{}`\n", item.env_keys.join("`, `")));
                            }
                            msg.push_str(&format!("  Quick Add: `{}`\n\n", item.example));
                        }
                        msg.push_str("Tip: Add any server with `/mcp add <id>` (e.g. `/mcp add github` or `/mcp add sqlite app.db`)");
                        self.session.add_message(Message::system(msg));
                    }
                    "add" => {
                        if parts.len() < 3 && parts.get(1).map(|&p| p == "add").unwrap_or(false) && parts.len() < 3 {
                            let help = "Usage:\n  /mcp add <catalog-id> [args...]\n  /mcp add <custom-name> <command> [args...]\n\nExamples:\n  /mcp add github\n  /mcp add sqlite app.db\n  /mcp add filesystem .\n  /mcp add mytool python3 /path/server.py\n\nRun `/mcp catalog` to browse all vetted community templates.";
                            self.session.add_message(Message::system(help));
                        } else {
                            let target_name = parts[2];
                            let mut cfg = self.llm_client.get_config();

                            if let Some(cat) = corex_tools::find_catalog_item(target_name) {
                                let mut args: Vec<String> = cat.default_args.iter().map(|s| s.to_string()).collect();
                                if parts.len() > 3 {
                                    if cat.id == "sqlite" {
                                        args = vec!["-y".into(), "@modelcontextprotocol/server-sqlite".into(), "--db-path".into(), parts[3].to_string()];
                                    } else if cat.id == "filesystem" {
                                        args = vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), parts[3].to_string()];
                                    } else if cat.id == "postgres" {
                                        args = vec!["-y".into(), "@modelcontextprotocol/server-postgres".into(), parts[3].to_string()];
                                    } else {
                                        args.extend(parts[3..].iter().map(|s| s.to_string()));
                                    }
                                }
                                let mcp_cfg = corex_core::McpServerConfig {
                                    command: cat.command.to_string(),
                                    args,
                                    env: std::collections::HashMap::new(),
                                };
                                let server_name = cat.id.to_string();
                                if let Err(e) = cfg.add_mcp_server(server_name.clone(), mcp_cfg, Some(&self.workspace_dir)) {
                                    self.session.add_message(Message::system(format!("Failed to save MCP config: {}", e)));
                                } else {
                                    self.llm_client.update_config(cfg);
                                    let statuses = self.reload_mcp_servers().await;
                                    let mut msg = format!("✓ Added MCP server '{}' from catalog.\n", cat.name);
                                    if !cat.env_keys.is_empty() {
                                        msg.push_str(&format!("Environment variables to set if required:\n  {}\n\n", cat.env_keys.iter().map(|k| format!("export {}=\"...\"", k)).collect::<Vec<_>>().join("\n  ")));
                                    }
                                    if let Some(s) = statuses.iter().find(|s| s.name == server_name) {
                                        let icon = if s.is_connected { "[Connected]" } else { "[Offline/Pending]" };
                                        msg.push_str(&format!("Status: {} — {} tools discovered", icon, s.tools_count));
                                        if let Some(ref err) = s.error {
                                            msg.push_str(&format!("\nWarning: {}", err));
                                        }
                                    }
                                    self.session.add_message(Message::system(msg));
                                }
                            } else if parts.len() >= 4 {
                                let server_name = target_name.to_string();
                                let command = parts[3].to_string();
                                let args = parts[4..].iter().map(|s| s.to_string()).collect();
                                let mcp_cfg = corex_core::McpServerConfig {
                                    command,
                                    args,
                                    env: std::collections::HashMap::new(),
                                };
                                if let Err(e) = cfg.add_mcp_server(server_name.clone(), mcp_cfg, Some(&self.workspace_dir)) {
                                    self.session.add_message(Message::system(format!("Failed to save MCP config: {}", e)));
                                } else {
                                    self.llm_client.update_config(cfg);
                                    let statuses = self.reload_mcp_servers().await;
                                    let mut msg = format!("✓ Added custom MCP server '{}'.\n", server_name);
                                    if let Some(s) = statuses.iter().find(|s| s.name == server_name) {
                                        let icon = if s.is_connected { "[Connected]" } else { "[Offline/Pending]" };
                                        msg.push_str(&format!("Status: {} — {} tools discovered", icon, s.tools_count));
                                        if let Some(ref err) = s.error {
                                            msg.push_str(&format!("\nError: {}", err));
                                        }
                                    }
                                    self.session.add_message(Message::system(msg));
                                }
                            } else {
                                self.session.add_message(Message::system(format!(
                                    "Unknown catalog template '{}'.\nFor custom commands, provide command and args:\n  /mcp add {} <command> [args...]\nOr browse available templates with `/mcp catalog`.",
                                    target_name, target_name
                                )));
                            }
                        }
                    }
                    "remove" | "delete" => {
                        if parts.len() < 3 {
                            self.session.add_message(Message::system("Usage: /mcp remove <name>"));
                        } else {
                            let name = parts[2];
                            let mut cfg = self.llm_client.get_config();
                            match cfg.remove_mcp_server(name, Some(&self.workspace_dir)) {
                                Ok(true) => {
                                    self.llm_client.update_config(cfg);
                                    let _ = self.reload_mcp_servers().await;
                                    self.session.add_message(Message::system(format!("✓ Removed MCP server '{}' and reloaded tools.", name)));
                                }
                                Ok(false) => {
                                    self.session.add_message(Message::system(format!("MCP server '{}' was not found in config.", name)));
                                }
                                Err(e) => {
                                    self.session.add_message(Message::system(format!("Failed to update MCP config: {}", e)));
                                }
                            }
                        }
                    }
                    "reload" => {
                        let statuses = self.reload_mcp_servers().await;
                        let mut msg = format!("Reloaded MCP Servers ({} configured):\n", statuses.len());
                        if statuses.is_empty() {
                            msg.push_str("  No servers configured. Type `/mcp catalog` to see available servers or `/mcp add <id>`.\n");
                        }
                        for s in &statuses {
                            let icon = if s.is_connected { "[OK]" } else { "[ERR]" };
                            msg.push_str(&format!("{} {} ({}): {} tools discovered\n", icon, s.name, s.command, s.tools_count));
                            if let Some(ref e) = s.error {
                                msg.push_str(&format!("   Error: {}\n", e));
                            }
                        }
                        self.session.add_message(Message::system(msg));
                    }
                    _ => {
                        let cfg = self.llm_client.get_config();
                        if cfg.mcp_servers.is_empty() {
                            let help = "No MCP (Model Context Protocol) servers configured.\n\n\
                                Quick start:\n\
                                • Browse templates:  /mcp catalog\n\
                                • Add GitHub MCP:    /mcp add github\n\
                                • Add SQLite MCP:    /mcp add sqlite app.db\n\
                                • Add Filesystem:    /mcp add filesystem .\n\
                                • Add custom:        /mcp add <name> <command> [args...]\n\n\
                                All servers are persisted in ~/.corex/settings.json or .corex/settings.json.";
                            self.session.add_message(Message::system(help));
                        } else {
                            let mut msg = format!("Configured MCP Servers ({}):\n", cfg.mcp_servers.len());
                            for (name, scfg) in &cfg.mcp_servers {
                                msg.push_str(&format!("• {}: {} {}\n", name, scfg.command, scfg.args.join(" ")));
                            }
                            msg.push_str("\nCommands:\n  /mcp catalog        - Browse available templates\n  /mcp add <name>     - Add new MCP server\n  /mcp remove <name>  - Remove an MCP server\n  /mcp reload         - Reload and reconnect servers");
                            self.session.add_message(Message::system(msg));
                        }
                    }
                }
                true
            }
            "/tasks" | "/background" => {
                let mgr = corex_tools::background::get_task_manager();
                if parts.len() == 1 {
                    let guard = match mgr.lock() {
                        Ok(g) => g,
                        Err(e) => e.into_inner(),
                    };
                    let list = guard.list();
                    self.session.add_message(Message::system(format!("Background Tasks:\n\n{}", list)));
                } else {
                    let sub = parts[1].to_lowercase();
                    match sub.as_str() {
                        "list" => {
                            let guard = match mgr.lock() {
                                Ok(g) => g,
                                Err(e) => e.into_inner(),
                            };
                            let list = guard.list();
                            self.session.add_message(Message::system(format!("Background Tasks:\n\n{}", list)));
                        }
                        "status" | "log" | "output" => {
                            if parts.len() > 2 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let guard = match mgr.lock() {
                                        Ok(g) => g,
                                        Err(e) => e.into_inner(),
                                    };
                                    match guard.get_status(pid) {
                                        Some(st) => self.session.add_message(Message::system(st)),
                                        None => self.session.add_message(Message::system(format!("Task {} not found.", pid))),
                                    }
                                } else {
                                    self.session.add_message(Message::system("Invalid Task ID / PID. Usage: /tasks status <pid>"));
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks status <pid>"));
                            }
                        }
                        "kill" | "stop" => {
                            if parts.len() > 2 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let mut guard = match mgr.lock() {
                                        Ok(g) => g,
                                        Err(e) => e.into_inner(),
                                    };
                                    if guard.kill(pid) {
                                        self.session.add_message(Message::system(format!("Terminated background task {}.", pid)));
                                    } else {
                                        self.session.add_message(Message::system(format!("Task {} not found.", pid)));
                                    }
                                } else {
                                    self.session.add_message(Message::system("Invalid Task ID / PID. Usage: /tasks kill <pid>"));
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks kill <pid>"));
                            }
                        }
                        "send" | "input" => {
                            if parts.len() > 3 {
                                if let Ok(pid) = parts[2].parse::<u32>() {
                                    let input = parts[3..].join(" ");
                                    let guard = match mgr.lock() {
                                        Ok(g) => g,
                                        Err(e) => e.into_inner(),
                                    };
                                    match guard.send_input(pid, input) {
                                        Ok(true) => self.session.add_message(Message::system(format!("Sent input to task {}.", pid))),
                                        _ => self.session.add_message(Message::system(format!("Failed to send input to task {}. (Task might be inactive)", pid))),
                                    }
                                } else {
                                    self.session.add_message(Message::system("Invalid Task ID / PID. Usage: /tasks send <pid> <input>"));
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks send <pid> <input text>"));
                            }
                        }
                        _ => {
                            if let Ok(pid) = parts[1].parse::<u32>() {
                                let guard = match mgr.lock() {
                                    Ok(g) => g,
                                    Err(e) => e.into_inner(),
                                };
                                match guard.get_status(pid) {
                                    Some(st) => self.session.add_message(Message::system(st)),
                                    None => self.session.add_message(Message::system(format!("Task {} not found.", pid))),
                                }
                            } else {
                                self.session.add_message(Message::system("Usage: /tasks [list] | /tasks status <pid> | /tasks kill <pid> | /tasks send <pid> <input>"));
                            }
                        }
                    }
                }
                true
            }
            "/yolo" => {
                if parts.len() > 1 {
                    let sub = parts[1].to_lowercase();
                    match sub.as_str() {
                        "on" | "enable" | "true" => {
                            self.always_allow_tools = true;
                            self.session.add_message(Message::system(YOLO_ON_MESSAGE));
                        }
                        "off" | "disable" | "false" => {
                            self.always_allow_tools = false;
                            self.session.add_message(Message::system(YOLO_OFF_MESSAGE));
                        }
                        _ => {
                            self.session.add_message(Message::system("Usage: /yolo on | /yolo off"));
                        }
                    }
                } else {
                    self.always_allow_tools = !self.always_allow_tools;
                    if self.always_allow_tools {
                        self.session.add_message(Message::system(YOLO_ON_MESSAGE));
                    } else {
                        self.session.add_message(Message::system(YOLO_OFF_MESSAGE));
                    }
                }
                true
            }

            "/fleet" | "/sysadmin" | "/vps" => {
                let cfg = corex_fleet::FleetConfig::load(Some(&self.workspace_dir)).unwrap_or_default();
                let nodes = cfg.all_nodes();
                if parts.len() > 1 && parts[1] == "wide" {
                    self.fleet_dialog.wide_layout = !self.fleet_dialog.wide_layout;
                    if !self.fleet_dialog.is_open {
                        self.enter_fleet_mode();
                        self.fleet_dialog.open_async(&self.workspace_dir).await;
                    }
                    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
                    if self.fleet_dialog.wide_layout {
                        if cols < crate::fleet_dialog::FLEET_WIDE_MIN_WIDTH {
                            self.session.add_message(Message::system(format!(
                                "Fleet 3-column wide layout enabled, but current terminal width is {} cols (< {} required). Maximizing your terminal window will automatically activate the 3-column view.",
                                cols,
                                crate::fleet_dialog::FLEET_WIDE_MIN_WIDTH
                            )));
                        } else {
                            self.session.add_message(Message::system(
                                "Fleet 3-column wide layout enabled (Chat · Live Terminal · Fleet Telemetry)."
                            ));
                        }
                    } else {
                        self.session.add_message(Message::system(
                            "Fleet wide layout disabled (standard view restored)."
                        ));
                    }
                    self.scroll_to_bottom();
                } else if parts.len() > 1 && matches!(parts[1], "off" | "close" | "exit" | "quit") {
                    self.exit_fleet_mode();
                } else if parts.len() > 1 && (parts[1] == "resume" || parts[1] == "sessions" || parts[1] == "history") {
                    self.enter_fleet_mode();
                    self.session_dialog.open(&self.workspace_dir.display().to_string(), true);
                } else if parts.len() == 1 || (parts.len() > 1 && (parts[1] == "nodes" || parts[1] == "list" || parts[1] == "dashboard" || parts[1] == "console")) {
                    self.enter_fleet_mode();
                    self.fleet_dialog.open_async(&self.workspace_dir).await;
                } else if parts.len() > 2 && (parts[1] == "connect" || parts[1] == "select") {
                    let target_id = parts[2];
                    self.enter_fleet_mode();
                    self.fleet_dialog.open_async(&self.workspace_dir).await;
                    if let Some(pos) = self.fleet_dialog.items.iter().position(|it| it.config.id == target_id) {
                        self.fleet_dialog.selected_idx = pos;
                        let node = self.fleet_dialog.items[pos].config.clone();
                        let mut session = corex_fleet::RemoteSession::new(node.clone());
                        match session.connect(8).await {
                            Ok(lat) => {
                                corex_fleet::global_session_manager().set_session(node.id.clone(), session.clone()).await;
                                corex_fleet::global_session_manager().set_active_node(Some(node.id.clone())).await;
                                self.fleet_dialog.active_session = Some(session);
                                match crate::fleet_pty::FleetPtySession::spawn(&node, 24, 80) {
                                    Ok(pty) => {
                                        self.fleet_dialog.pty_session = Some(pty);
                                        self.fleet_dialog.status_msg = Some(format!("Connected to {} in {}ms", node.id, lat));
                                    }
                                    Err(err) => {
                                        self.fleet_dialog.pty_session = None;
                                        self.fleet_dialog.status_msg = Some(format!("Connected to {} ({}ms); interactive PTY unavailable ({}). Fallback batch SSH mode active.", node.id, lat, err));
                                    }
                                }
                                self.session.add_message(Message::system(format!(
                                    "Connected to remote server: {} ({}@{}) · Active and ready.",
                                    node.id, node.user, node.host
                                )));
                                self.scroll_to_bottom();
                            }
                            Err(e) => {
                                self.fleet_dialog.status_msg = Some(format!("Could not connect to {}: {}", node.id, e));
                            }
                        }
                    } else {
                        self.fleet_dialog.status_msg = Some(format!("Server '{}' not found in inventory", target_id));
                    }
                } else if parts.len() > 1 && parts[1] == "status" {
                    let mut msg = format!("Fleet Nodes ({} servers):\n", nodes.len());
                    for n in &nodes {
                        msg.push_str(&format!("• [{}] {}\n", n.id, n.target_string()));
                    }
                    msg.push_str("\n(For full real-time telemetry run `cx fleet telemetry` or ask the assistant: 'check server status')");
                    self.session.add_message(Message::system(msg));
                } else {
                    self.enter_fleet_mode();
                    self.fleet_dialog.open_async(&self.workspace_dir).await;
                }
                true
            }
            "/help" => {
                let mut help = "Available Commands:\n".to_string();
                let mut last_group = "";
                for c in ALL_COMMANDS {
                    if c.group != last_group {
                        last_group = c.group;
                        help.push_str(&format!("\n {}\n", c.group));
                    }
                    let usage = if c.args.is_empty() {
                        c.name.to_string()
                    } else {
                        format!("{} {}", c.name, c.args)
                    };
                    let alias = if c.aliases.is_empty() {
                        String::new()
                    } else {
                        format!(" (alias {})", c.aliases.join(", "))
                    };
                    help.push_str(&format!("  {:<30} {}{}\n", usage, c.description, alias));
                }
                help.push_str("  /sudo <pwd>  Store sudo password in RAM for silent privilege escalation\n");
                help.push_str("\nKeyboard Shortcuts:\n  Enter: Submit | Ctrl+T: Toggle Thought Box | Ctrl+C: Cancel/Exit | Up/Down: History / Command Nav");
                help.push_str("\n  Scroll chat: Mouse wheel | PageUp / PageDown | Shift+Up / Shift+Down");
                help.push_str("\n  Copy text: drag to select, release to copy and clear the highlight");
                self.session.add_message(Message::system(help));
                true
            }
            _ => false,
        }
    }

    /// Adds a read-only root for this session, after checking it is a real, non-sensitive
    /// directory the agent cannot already read.
    pub(super) fn add_directory(&mut self, raw: &str) -> Result<PathBuf, String> {
        use corex_tools::fs_tools::{is_sensitive_system_path, is_within_workspace, resolve_path};

        let dir = resolve_path(&self.workspace_dir, raw).canonicalize().map_err(|e| e.to_string())?;
        if !dir.is_dir() {
            return Err("not a directory".to_string());
        }
        if is_sensitive_system_path(&dir) {
            return Err("it holds credentials or system files".to_string());
        }
        if std::iter::once(&self.workspace_dir).chain(&self.extra_dirs).any(|root| is_within_workspace(&dir, root)) {
            return Err("it is already readable".to_string());
        }
        self.extra_dirs.push(dir.clone());
        Ok(dir)
    }

    pub(super) fn directory_report(&self) -> String {
        let mut msg = format!("Workspace: {}", self.workspace_dir.display());
        if self.extra_dirs.is_empty() {
            msg.push_str("\nNo additional directories. Add one with /directory add <path>.");
        } else {
            msg.push_str("\nAdditional directories (read without confirmation, for this session):");
            for dir in &self.extra_dirs {
                msg.push_str(&format!("\n- {}", dir.display()));
            }
        }
        msg
    }
}

/// Loaded instruction files in prompt order, followed by the full search order.
fn memory_file_list(memory: &Memory) -> String {
    let files = memory.files();
    let mut msg = if files.is_empty() {
        "No instruction files loaded.".to_string()
    } else {
        format!("Instruction files in the system prompt ({}, in order):", files.len())
    };
    for (i, file) in files.iter().enumerate() {
        let scope = match file.scope {
            MemoryScope::Global => "global",
            MemoryScope::Project => "project",
        };
        msg.push_str(&format!(
            "\n{}. [{}] {} — {} bytes",
            i + 1,
            scope,
            file.display,
            group_thousands(file.original_len as u64)
        ));
        if file.is_truncated() {
            msg.push_str(&format!(" (truncated to {})", group_thousands(file.content.len() as u64)));
        }
    }
    msg.push_str("\n\nSearch order (all found files are loaded):");
    for (_, path) in memory.candidates() {
        let mark = if files.iter().any(|f| f.path == path) { "✓" } else { "·" };
        msg.push_str(&format!("\n{} {}", mark, memory.display_path(&path)));
    }
    msg
}
