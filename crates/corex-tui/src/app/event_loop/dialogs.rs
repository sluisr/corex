//! Keys for the modal dialogs: sudo password, API key, session picker, tool approval and the
//! ask_user questions. Each handler returns `Flow::Pass` when its dialog is closed.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use corex_core::client::set_sudo_password;
use corex_core::session::Session;
use corex_core::types::Message;
use corex_tools::types::ToolContext;

use super::tools::{finish_ask_user, first_sudo_command, run_single_tool, run_tools_in_parallel, show_next_call, tool_context};
use super::{EventTx, Flow};
use crate::app::formatting::extract_tool_allow_rules;
use crate::app::state::App;

pub(super) fn sudo_key(app: &mut App, key: KeyEvent, event_tx: &EventTx) -> Flow {
    if !app.sudo_dialog.is_open {
        return Flow::Pass;
    }
    match key.code {
        KeyCode::Esc => {
            for call in take_sudo_calls(app) {
                app.session.add_message(Message::tool_response(
                    call.id,
                    "Sudo authentication cancelled by user.",
                ));
            }
            app.sudo_dialog.close();
            if app.pending_confirmation.is_none() && app.in_flight_tool_calls == 0 {
                app.start_stream_turn(event_tx.clone());
            }
        }
        KeyCode::Backspace => {
            app.sudo_dialog.password_input.pop();
        }
        KeyCode::Char(c) => {
            app.sudo_dialog.password_input.push(c);
        }
        KeyCode::Enter => {
            let pwd = app.sudo_dialog.password_input.trim().to_string();
            if pwd.is_empty() {
                app.sudo_dialog.error_msg = Some("Password cannot be empty.".to_string());
            } else {
                // Sudo password is not persisted across commands
                set_sudo_password(None);
                let calls = take_sudo_calls(app);
                if !calls.is_empty() {
                    let context = ToolContext { sudo_password: Some(pwd), ..tool_context(app) };
                    app.is_streaming = true;
                    run_tools_in_parallel(app, calls, context, event_tx.clone());
                }
                app.sudo_dialog.close();
            }
        }
        _ => {}
    }
    Flow::Handled
}

/// The calls waiting for the sudo password, removed from the dialog.
fn take_sudo_calls(app: &mut App) -> Vec<corex_core::types::ToolCall> {
    if !app.sudo_dialog.pending_calls.is_empty() {
        std::mem::take(&mut app.sudo_dialog.pending_calls)
    } else {
        app.sudo_dialog.pending_call.take().into_iter().collect()
    }
}

pub(super) fn auth_key(app: &mut App, key: KeyEvent) -> Flow {
    if !app.auth_dialog.is_open {
        return Flow::Pass;
    }
    match key.code {
        KeyCode::Esc => {
            app.auth_dialog.close();
            app.session.add_message(Message::system(
                "Auth dialog cancelled. You can set keys anytime via /model, /key, or run offline via /local."
            ));
        }
        KeyCode::Backspace => {
            app.auth_dialog.input_buffer.pop();
        }
        KeyCode::Char(c) => {
            app.auth_dialog.input_buffer.push(c);
        }
        KeyCode::Enter => {
            let key_str = app.auth_dialog.input_buffer.trim().to_string();
            if key_str.is_empty() {
                app.auth_dialog.error_msg = Some("API key cannot be empty.".to_string());
            } else {
                save_api_key(app, key_str);
            }
        }
        _ => {}
    }
    Flow::Handled
}

/// Saves the key typed in the auth dialog for its provider and activates the model that
/// asked for it, if any.
fn save_api_key(app: &mut App, key_str: String) {
    let env_var = app.auth_dialog.api_key_env.clone();
    let prov_name = app.auth_dialog.provider_name.clone();
    let prov_label = app.auth_dialog.provider_label.clone();
    let pending = app.auth_dialog.pending_activation;

    let mut cfg = app.llm_client.get_config();
    if prov_name == "deepseek" {
        cfg.api_key = key_str.clone();
    } else {
        cfg.provider_api_keys.insert(prov_name.clone(), key_str.clone());
    }
    let _ = cfg.save_with_workspace(Some(&app.workspace_dir));
    app.llm_client.update_config(cfg);
    corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(app.llm_client.clone()));
    if let Some(pos) = app.model_dialog.providers.iter().position(|p| p.name == prov_name) {
        if pos < app.model_dialog.ready.len() {
            app.model_dialog.ready[pos] = true;
        }
    }
    app.auth_dialog.close();
    let note = crate::auth_dialog::env_override_note(&prov_name, &key_str).unwrap_or_default();
    app.session.add_message(Message::system(format!(
        "API key saved for {} (${})!{}",
        prov_label, env_var, note
    )));

    if let Some((p_idx, m_idx)) = pending {
        app.activate_model(p_idx, m_idx);
    }
}

pub(super) fn session_key(app: &mut App, key: KeyEvent) -> Flow {
    if !app.session_dialog.is_open {
        return Flow::Pass;
    }
    let filtered_count = app.session_dialog.filtered_sessions().len();

    // A delete is waiting for its answer: `y` confirms, any other key backs out.
    if app.session_dialog.pending_delete.is_some() {
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            if let Some(session_id) = app.session_dialog.take_pending_delete() {
                let _ = Session::delete_by_id_or_tag(&session_id);
                app.session_dialog.reload(&app.workspace_dir.display().to_string());
            }
        } else {
            app.session_dialog.pending_delete = None;
        }
        return Flow::Handled;
    }

    match key.code {
        KeyCode::Esc => {
            if !app.session_dialog.search_query.is_empty() {
                app.session_dialog.search_query.clear();
                app.session_dialog.selected_idx = 0;
            } else {
                app.session_dialog.close();
            }
        }
        KeyCode::Backspace => {
            app.session_dialog.search_query.pop();
            app.session_dialog.selected_idx = 0;
        }
        KeyCode::Up => {
            if app.session_dialog.selected_idx > 0 {
                app.session_dialog.selected_idx -= 1;
            }
        }
        KeyCode::Down => {
            if filtered_count > 0 && app.session_dialog.selected_idx + 1 < filtered_count {
                app.session_dialog.selected_idx += 1;
            }
        }
        // Delete only through keys that cannot be typed into the filter: a plain
        // letter belongs to the search box, and the removal asks to be confirmed.
        KeyCode::Delete => {
            app.session_dialog.request_delete();
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.session_dialog.request_delete();
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
            app.session_dialog.search_query.push(c);
            app.session_dialog.selected_idx = 0;
        }
        KeyCode::Enter => {
            if let Some(selected_session) = app.session_dialog.selected_session().cloned() {
                resume_session(app, &selected_session.id);
            }
            app.session_dialog.close();
        }
        _ => {}
    }
    Flow::Handled
}

/// Loads the session `session_id` picked in the dialog, with its model settings.
fn resume_session(app: &mut App, session_id: &str) {
    match Session::load_by_id_or_tag(session_id) {
        Ok(mut loaded) => {
            let mut cfg = app.llm_client.get_config();
            cfg.model = loaded.model.clone();
            if let Some(t) = loaded.temperature {
                cfg.temperature = t;
            }
            if let Some(ref r) = loaded.reasoning_effort {
                cfg.reasoning_effort = r.clone();
            }
            app.llm_client.update_config(cfg);
            corex_tools::subagent::set_subagent_llm_client(std::sync::Arc::new(app.llm_client.clone()));
            let is_fleet = app.session_dialog.is_fleet;
            if is_fleet {
                loaded.tag = Some("fleet".to_string());
            }
            app.session = loaded;
            app.cached_session_id.clear();
            app.invalidate_message_cache();
            app.clear_selection();
            app.scroll_to_bottom();
            let label = if is_fleet { "Fleet session" } else { "session" };
            app.session.add_message(Message::system(format!(
                "Resumed {} '{}' (Model: {}).",
                label, app.session.title, app.session.model
            )));
        }
        Err(e) => {
            app.session.add_message(Message::system(format!("Failed to load session: {}", e)));
        }
    }
}

/// What a key means in the tool approval dialog. The numbers follow the options as listed:
/// a batch of several calls offers "allow all", a single call "allow always" instead.
struct ApprovalAnswer {
    allow_single: bool,
    allow_all: bool,
    allow_session: bool,
    allow_always: bool,
    deny: bool,
    feedback: bool,
}

impl ApprovalAnswer {
    fn from_key(code: KeyCode, selected: usize, is_multi: bool) -> Self {
        let chosen = |digit: char, letters: &[char], option: usize| {
            code == KeyCode::Char(digit)
                || matches!(code, KeyCode::Char(c) if letters.contains(&c))
                || (code == KeyCode::Enter && selected == option)
        };
        Self {
            allow_single: chosen('1', &['y', 'Y'], 0),
            allow_all: is_multi && chosen('2', &['a', 'A'], 1),
            allow_session: if is_multi { chosen('3', &['s', 'S'], 2) } else { chosen('2', &['s', 'S'], 1) },
            allow_always: !is_multi && chosen('4', &['p', 'P'], 3),
            deny: code == KeyCode::Esc || if is_multi { chosen('4', &['n', 'N'], 3) } else { chosen('3', &['n', 'N'], 2) },
            feedback: chosen('5', &['w', 'W'], 4),
        }
    }
}

/// The tool approval dialog. Keys it does not use reach the prompt (`Flow::Pass`).
pub(super) fn confirmation_key(app: &mut App, key: KeyEvent, event_tx: &EventTx) -> Flow {
    let Some(pending) = app.pending_confirmation.as_mut() else {
        return Flow::Pass;
    };
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let is_ctrl_o = (is_ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')))
        || key.code == KeyCode::Char('\x0f');

    if is_ctrl_o {
        pending.diff_expanded = !pending.diff_expanded;
        app.logs_expanded = pending.diff_expanded;
        app.invalidate_message_cache();
        app.auto_scroll = true;
        return Flow::Handled;
    }

    if pending.input_mode {
        match key.code {
            KeyCode::Esc => {
                pending.input_mode = false;
                pending.feedback_text.clear();
            }
            KeyCode::Backspace => {
                pending.feedback_text.pop();
            }
            KeyCode::Char(c) if !is_ctrl && !c.is_control() => {
                pending.feedback_text.push(c);
            }
            KeyCode::Enter => {
                let feedback = pending.feedback_text.trim().to_string();
                deny_batch_with_feedback(app, &feedback, event_tx);
            }
            _ => {}
        }
        return Flow::Handled;
    }

    match key.code {
        KeyCode::Up => {
            pending.selected_option = pending.selected_option.saturating_sub(1);
            return Flow::Handled;
        }
        KeyCode::Down => {
            pending.selected_option = (pending.selected_option + 1).min(4);
            return Flow::Handled;
        }
        _ => {}
    }

    let answer = ApprovalAnswer::from_key(key.code, pending.selected_option, pending.total_count > 1);

    if answer.feedback {
        pending.input_mode = true;
        pending.selected_option = 4;
        return Flow::Handled;
    }

    if answer.deny {
        deny_current_call(app, event_tx);
        return Flow::Handled;
    }

    if answer.allow_session {
        allow_for_session(app);
    }
    if answer.allow_always {
        allow_always(app);
    }

    if answer.allow_single && approve_current_call(app, event_tx) {
        return Flow::Handled;
    }

    if answer.allow_single || answer.allow_all || answer.allow_session || answer.allow_always {
        let only_first = (answer.allow_session || answer.allow_always) && !answer.allow_all;
        run_approved(app, only_first, event_tx);
        return Flow::Handled;
    }
    Flow::Pass
}

/// [w] then Enter: denies every call left in the batch, passing the user's instruction on.
fn deny_batch_with_feedback(app: &mut App, feedback: &str, event_tx: &EventTx) {
    let Some(pending_batch) = app.pending_confirmation.take() else {
        return;
    };
    let reason = if feedback.is_empty() {
        "Tool execution denied by user.".to_string()
    } else {
        format!("Tool execution denied by user with instruction: {}", feedback)
    };
    for call in pending_batch.calls {
        app.session.add_message(Message::tool_response(call.id, reason.clone()));
    }
    if app.in_flight_tool_calls == 0 && !app.sudo_dialog.is_open {
        app.start_stream_turn(event_tx.clone());
    }
}

/// [n]: denies the call on screen and moves on to the next one, if any.
fn deny_current_call(app: &mut App, event_tx: &EventTx) {
    let Some(mut pending_batch) = app.pending_confirmation.take() else {
        return;
    };
    if pending_batch.calls.is_empty() {
        return;
    }
    let call = pending_batch.calls.remove(0);
    let mut tool_msg = Message::tool_response(call.id, "Tool execution denied by user.".to_string());
    tool_msg.name = Some(call.function.name);
    app.session.add_message(tool_msg);
    let _ = app.session.save();

    if pending_batch.calls.is_empty() {
        if app.in_flight_tool_calls == 0 && !app.sudo_dialog.is_open {
            app.start_stream_turn(event_tx.clone());
        }
    } else {
        show_next_call(app, &mut pending_batch, 1);
        app.pending_confirmation = Some(pending_batch);
    }
}

/// [y] on a batch of several calls: runs the call on screen and shows the next one. Returns
/// false for the last call of a batch, which runs through [`run_approved`] like "allow all".
fn approve_current_call(app: &mut App, event_tx: &EventTx) -> bool {
    let Some(pending_batch) = app.pending_confirmation.as_mut() else {
        return false;
    };
    if pending_batch.calls.len() <= 1 {
        return false;
    }
    let mut pending_batch = app.pending_confirmation.take().expect("checked above");
    let call = pending_batch.calls.remove(0);
    show_next_call(app, &mut pending_batch, 1);
    app.pending_confirmation = Some(pending_batch);

    match first_sudo_command(std::slice::from_ref(&call)) {
        Some(sudo_cmd) => app.sudo_dialog.open_batch(vec![call], sudo_cmd),
        None => run_single_tool(app, call, event_tx.clone()),
    }
    true
}

/// Runs the approved calls: the whole batch, or with `only_first` the call on screen plus any
/// other that the rule just allowed no longer needs approval. The rest stay in the dialog.
fn run_approved(app: &mut App, only_first: bool, event_tx: &EventTx) {
    let Some(mut pending_batch) = app.pending_confirmation.take() else {
        return;
    };
    let context = tool_context(app);

    let calls = if only_first && pending_batch.calls.len() > 1 {
        let mut approved = Vec::new();
        let mut remaining = Vec::new();
        for (idx, c) in pending_batch.calls.into_iter().enumerate() {
            let args_json = corex_core::parse_lenient_json(&c.function.arguments)
                .unwrap_or(serde_json::Value::Null);
            let still_needs_confirm = idx > 0
                && app
                    .tool_registry
                    .get(&c.function.name)
                    .map(|t| t.needs_confirmation(&args_json, &context))
                    .unwrap_or(true);
            if still_needs_confirm {
                remaining.push(c);
            } else {
                approved.push(c);
            }
        }
        if !remaining.is_empty() {
            pending_batch.calls = remaining;
            show_next_call(app, &mut pending_batch, approved.len());
            app.pending_confirmation = Some(pending_batch);
        }
        approved
    } else {
        pending_batch.calls
    };

    if let Some(sudo_cmd) = first_sudo_command(&calls) {
        app.sudo_dialog.open_batch(calls, sudo_cmd);
        return;
    }

    app.is_streaming = true;
    app.last_turn_start = Some(std::time::Instant::now());
    run_tools_in_parallel(app, calls, context, event_tx.clone());
}

/// [s]: allows the rules of the call on screen until the session ends.
fn allow_for_session(app: &mut App) {
    let mut added_rules = Vec::new();
    if let Some(call) = app.pending_confirmation.as_ref().and_then(|pb| pb.calls.first()) {
        for rule in extract_tool_allow_rules(call) {
            if !rule.is_empty() && !app.session_allowed_commands.contains(&rule) {
                app.session_allowed_commands.push(rule.clone());
                added_rules.push(rule);
            }
        }
    }
    if !added_rules.is_empty() {
        app.session.add_message(Message::system(format!(
            "Allowed for this session: {}",
            describe_rules(&added_rules)
        )));
    }
}

/// [p]: saves the rules of the call on screen to `allowed_commands` in settings.json.
fn allow_always(app: &mut App) {
    let mut cfg = app.llm_client.get_config();
    let mut added_rules = Vec::new();
    if let Some(call) = app.pending_confirmation.as_ref().and_then(|pb| pb.calls.first()) {
        for rule in extract_tool_allow_rules(call) {
            if !rule.is_empty() && !cfg.allowed_commands.contains(&rule) {
                cfg.allowed_commands.push(rule.clone());
                added_rules.push(rule);
            }
        }
    }

    if let Err(e) = cfg.save_with_workspace(Some(&app.workspace_dir)) {
        app.session.add_message(Message::system(format!(
            "Warning: Could not save allowed_commands to config: {}",
            e
        )));
    } else if !added_rules.is_empty() {
        app.session.add_message(Message::system(format!(
            "Added to ~/.corex/settings.json allowed_commands: {}",
            describe_rules(&added_rules)
        )));
    }
    app.llm_client.update_config(cfg);
}

/// How newly allowed rules are named to the user.
fn describe_rules(rules: &[String]) -> String {
    if rules.iter().any(|r| r == "apply_patch") && rules.iter().any(|r| r == "edit") {
        "file modifications (apply_patch, edit, write_file)".to_string()
    } else {
        rules.join(", ")
    }
}

pub(super) fn rewind_key(app: &mut App, key: KeyEvent) -> Flow {
    if !app.rewind_dialog.is_open {
        return Flow::Pass;
    }
    match key.code {
        KeyCode::Esc => {
            if app.rewind_dialog.back() {
                app.rewind_dialog.close();
            }
        }
        KeyCode::Up => app.rewind_dialog.up(),
        KeyCode::Down => app.rewind_dialog.down(),
        KeyCode::Char(c) if app.rewind_dialog.confirming.is_some() && c.is_ascii_digit() => {
            let n = c.to_digit(10).unwrap_or(0) as usize;
            if (1..=app.rewind_dialog.options().len()).contains(&n) {
                app.rewind_dialog.option = n - 1;
                if let Some((action, turn)) = app.rewind_dialog.enter() {
                    app.apply_rewind(action, turn);
                }
            }
        }
        KeyCode::Enter => {
            if let Some((action, turn)) = app.rewind_dialog.enter() {
                app.apply_rewind(action, turn);
            }
        }
        _ => {}
    }
    Flow::Handled
}

pub(super) fn ask_user_key(app: &mut App, key: KeyEvent, event_tx: &EventTx) -> Flow {
    if !app.user_dialog.is_open {
        return Flow::Pass;
    }
    let cur = app.user_dialog.current;
    match key.code {
        KeyCode::Esc => {
            app.user_dialog.cancelled = true;
            finish_ask_user(app, event_tx.clone());
        }
        KeyCode::Up => {
            if let Some(q) = app.user_dialog.current_question() {
                if q.has_options && app.user_dialog.selected[cur] > 0 {
                    app.user_dialog.selected[cur] -= 1;
                }
            }
        }
        KeyCode::Down => {
            if let Some(q) = app.user_dialog.current_question() {
                if q.has_options {
                    let max = q.options.len().saturating_sub(1);
                    if app.user_dialog.selected[cur] < max {
                        app.user_dialog.selected[cur] += 1;
                    }
                }
            }
        }
        KeyCode::Backspace => {
            if !app.user_dialog.questions[cur].has_options {
                app.user_dialog.text_input[cur].pop();
            }
        }
        KeyCode::PageUp => app.scroll_chat(-10),
        KeyCode::PageDown => app.scroll_chat(10),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && !c.is_control() => {
            if let Some(q) = app.user_dialog.current_question() {
                if q.has_options {
                    // 1-9 pick an option.
                    if let Some(idx) = c.to_digit(10).filter(|d| *d >= 1).map(|d| (d - 1) as usize) {
                        if idx < q.options.len() {
                            app.user_dialog.selected[cur] = idx;
                        }
                    }
                } else {
                    app.user_dialog.text_input[cur].push(c);
                }
            }
        }
        KeyCode::Enter => {
            if cur + 1 < app.user_dialog.questions.len() {
                app.user_dialog.current += 1;
            } else {
                finish_ask_user(app, event_tx.clone());
            }
        }
        _ => {}
    }
    Flow::Handled
}
