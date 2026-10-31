//! The prompt line: editing, the slash command menu, history and sending a message.

use std::time::{Duration, Instant};

use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers};

use corex_core::client::set_sudo_password;

use super::{EventTx, Flow};
use crate::app::formatting::{paste_tag_last_start, starts_with_paste_tag};
use crate::app::mobile::arrows_may_be_swipes;
use crate::app::state::App;

pub(super) async fn prompt_key(app: &mut App, key: KeyEvent, event_tx: &EventTx) -> Flow {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let is_ctrl_p = (ctrl && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))) || key.code == KeyCode::Char('\x10');
    let is_ctrl_n = (ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N'))) || key.code == KeyCode::Char('\x0e');

    // Candidates are empty while browsing prompt history, so bare
    // Up/Down always keep walking the history instead of being
    // captured by the completion menu (e.g. recalled `/resume`).
    let matching_cmds = app.matching_slash_commands();
    let is_slash_open = !matching_cmds.is_empty();
    let matching_files = app.file_completions().unwrap_or_default();
    let is_files_open = !matching_files.is_empty();

    match key.code {
        // History via readline shortcuts: Ctrl+P / Alt+Up (previous). Before the `Char` arms,
        // which would otherwise take Ctrl+P as an unbound control key.
        _ if is_ctrl_p || (alt && key.code == KeyCode::Up) => {
            if is_slash_open {
                select_prev_command(app, matching_cmds.len());
            } else {
                history_prev(app);
            }
        }
        // Ctrl+N / Alt+Down (next)
        _ if is_ctrl_n || (alt && key.code == KeyCode::Down) => {
            if is_slash_open {
                app.slash_selected_idx = (app.slash_selected_idx + 1) % matching_cmds.len();
            } else {
                history_next(app);
            }
        }
        KeyCode::Left => {
            if ctrl || alt {
                app.cursor_idx = word_start(&app.input_buffer[..app.cursor_idx]);
            } else if app.cursor_idx > 0 {
                let before = &app.input_buffer[..app.cursor_idx];
                // A paste tag moves as one piece.
                app.cursor_idx = before
                    .ends_with(']')
                    .then(|| paste_tag_last_start(before))
                    .flatten()
                    .unwrap_or_else(|| prev_char_start(before));
            }
            app.clamp_cursor();
        }
        KeyCode::Right => {
            if ctrl || alt {
                let after = &app.input_buffer[app.cursor_idx..];
                let trimmed = after.trim_start();
                let skip_spaces = after.len() - trimmed.len();
                let word_len = trimmed.find(' ').unwrap_or(trimmed.len());
                app.cursor_idx = (app.cursor_idx + skip_spaces + word_len).min(app.input_buffer.len());
            } else if app.cursor_idx < app.input_buffer.len() {
                let after = &app.input_buffer[app.cursor_idx..];
                let step = starts_with_paste_tag(after)
                    .then(|| after.find(']').map(|end| end + 1))
                    .flatten()
                    .unwrap_or_else(|| next_char_len(after));
                app.cursor_idx = (app.cursor_idx + step).min(app.input_buffer.len());
            }
            app.clamp_cursor();
        }
        KeyCode::Home => {
            app.cursor_idx = 0;
        }
        KeyCode::End => {
            app.cursor_idx = app.input_buffer.len();
            if !app.auto_scroll {
                app.scroll_to_bottom();
            }
        }
        KeyCode::Delete => {
            app.clamp_cursor();
            if app.cursor_idx < app.input_buffer.len() {
                let after = &app.input_buffer[app.cursor_idx..];
                let tag_end = starts_with_paste_tag(after).then(|| after.find(']')).flatten();
                let len = match tag_end {
                    Some(end_bracket) => {
                        let tag = after[..=end_bracket].to_string();
                        app.forget_paste_tag(&tag);
                        end_bracket + 1
                    }
                    None => next_char_len(after),
                };
                app.input_buffer.drain(app.cursor_idx..app.cursor_idx + len);
                app.clamp_cursor();
            }
        }
        KeyCode::Char(c) if ctrl || c.is_control() => match c {
            'a' | 'A' | '\x01' => app.cursor_idx = 0,
            'e' | 'E' | '\x05' => app.cursor_idx = app.input_buffer.len(),
            'u' | 'U' | '\x15' => {
                app.input_buffer.drain(..app.cursor_idx);
                app.cursor_idx = 0;
            }
            'k' | 'K' | '\x0b' => {
                app.input_buffer.truncate(app.cursor_idx);
            }
            // Terminals that do not paste on their own pass Ctrl+V through: take
            // an image from the clipboard if there is one, else its text.
            'v' | 'V' | '\x16' => app.paste_from_clipboard(),
            'w' | 'W' | '\x17' => {
                let start = word_start(&app.input_buffer[..app.cursor_idx]);
                app.input_buffer.drain(start..app.cursor_idx);
                app.cursor_idx = start;
            }
            _ => {}
        },
        KeyCode::Char(c) => {
            if c == ' ' && app.input_buffer.is_empty() && !app.auto_scroll {
                app.scroll_to_bottom();
                return Flow::Handled;
            }
            app.clamp_cursor();
            app.input_buffer.insert(app.cursor_idx, c);
            app.cursor_idx += c.len_utf8();
            app.slash_selected_idx = 0;
            app.last_esc_press = None;
        }
        KeyCode::Backspace => {
            app.clamp_cursor();
            if app.cursor_idx > 0 {
                let before = &app.input_buffer[..app.cursor_idx];
                let tag_start = before
                    .ends_with(']')
                    .then(|| paste_tag_last_start(before))
                    .flatten()
                    .filter(|&open_idx| before[open_idx..].ends_with(']'));
                let start = match tag_start {
                    Some(open_idx) => {
                        let tag = before[open_idx..].to_string();
                        app.forget_paste_tag(&tag);
                        open_idx
                    }
                    None => prev_char_start(before),
                };
                app.input_buffer.drain(start..app.cursor_idx);
                app.cursor_idx = start;
            }
            app.clamp_cursor();
            app.slash_selected_idx = 0;
            app.last_esc_press = None;
        }
        KeyCode::Esc => {
            if is_slash_open {
                clear_prompt(app);
            } else if app.fleet_dialog.is_open && !app.fleet_dialog.command_mode {
                app.exit_fleet_mode();
            } else if !app.auto_scroll {
                // 1st ESC when scrolled up -> jump straight to bottom without modifying input buffer!
                app.scroll_to_bottom();
                app.last_esc_press = None;
            } else if app.last_esc_press.is_some_and(|i| i.elapsed() <= Duration::from_millis(500)) {
                // 2nd ESC within 500ms -> Clear prompt & reset history navigation!
                clear_prompt(app);
                app.history_idx = None;
                app.saved_draft.clear();
                app.slash_popup_height_current = 0.0;
            } else {
                // 1st ESC -> Start 500ms window to show toast
                app.last_esc_press = Some(Instant::now());
            }
        }
        KeyCode::Tab | KeyCode::Enter if is_files_open && !alt => {
            let selected = matching_files[app.slash_selected_idx % matching_files.len()].clone();
            app.accept_file_completion(&selected);
        }
        KeyCode::Up if is_files_open && !(ctrl || shift) => select_prev_command(app, matching_files.len()),
        KeyCode::Down if is_files_open && !(ctrl || shift) => {
            app.slash_selected_idx = (app.slash_selected_idx + 1) % matching_files.len();
        }
        KeyCode::Tab => {
            if is_slash_open {
                let selected = &matching_cmds[app.slash_selected_idx % matching_cmds.len()];
                app.input_buffer = format!("{} ", selected);
                app.cursor_idx = app.input_buffer.len();
            }
        }
        // Chat feed scrolling: Ctrl/Shift modifiers or PageUp/PageDown scroll chat.
        KeyCode::Up if ctrl || shift => app.scroll_chat(if shift { -10 } else { -3 }),
        KeyCode::Down if ctrl || shift => app.scroll_chat(if shift { 10 } else { 3 }),
        KeyCode::Up => {
            if is_slash_open {
                select_prev_command(app, matching_cmds.len());
            } else if arrow_is_swipe(app) {
                // Termux sends one arrow per row the finger moves: follow it 1:1.
                app.scroll_chat(-1);
            } else {
                history_prev(app);
            }
        }
        KeyCode::Down => {
            if is_slash_open {
                app.slash_selected_idx = (app.slash_selected_idx + 1) % matching_cmds.len();
            } else if arrow_is_swipe(app) {
                app.scroll_chat(1);
            } else {
                history_next(app);
            }
        }
        KeyCode::PageUp => app.scroll_chat(-10),
        KeyCode::PageDown => app.scroll_chat(10),
        KeyCode::Enter => {
            // Alt+Enter while the agent works: hand the message over at the next
            // safe point instead of queueing it behind the whole task.
            let send_now = alt && app.is_streaming;
            let text = app.input_buffer.trim().to_string();
            if text.is_empty() && send_now {
                send_pending_now(app, event_tx);
            } else if !text.is_empty() {
                // A trailing space means the typed command is complete: run it as is
                // (`/yolo ` toggles) instead of picking the first subcommand.
                let cmd_to_run = if is_slash_open && !app.input_buffer.ends_with(char::is_whitespace) {
                    matching_cmds[app.slash_selected_idx % matching_cmds.len()].clone()
                } else {
                    text
                };
                return submit(app, cmd_to_run, send_now, event_tx).await;
            }
        }
        _ => {}
    }
    app.refresh_file_index();
    Flow::Handled
}

/// Alt+Enter on an empty prompt while the agent works: sends the oldest queued message at the
/// next safe point, or, when one is already waiting there, interrupts the turn to deliver it.
fn send_pending_now(app: &mut App, event_tx: &EventTx) {
    if app.steer_messages.is_empty() {
        app.promote_queued_to_steer();
    } else {
        app.cancel_generation("Generation interrupted to deliver your message.");
        app.deliver_steer();
        app.scroll_to_bottom();
        app.start_stream_turn(event_tx.clone());
    }
}

/// Sends the prompt: runs a slash command, or expands paste tags and starts a turn (or queues
/// the message while one is running).
async fn submit(app: &mut App, cmd_to_run: String, send_now: bool, event_tx: &EventTx) -> Flow {
    let mut user_prompt_clean = strip_prompt_prefixes(cmd_to_run);

    // Record sanitized command to input history (NEVER leak a password or key to history on disk)
    let entry = history_entry(&user_prompt_clean);
    if app.input_history.last() != Some(&entry) && !entry.is_empty() {
        app.input_history.push(entry.clone());
        corex_core::HistoryStore::append(&entry);
    }
    app.history_idx = None;
    app.saved_draft.clear();
    app.input_buffer.clear();
    app.cursor_idx = 0;
    app.slash_selected_idx = 0;
    app.slash_popup_height_current = 0.0;

    if user_prompt_clean == "/quit" || user_prompt_clean == "/exit" {
        return Flow::Quit;
    }

    if app.is_streaming && replaces_conversation(&user_prompt_clean) {
        app.cancel_generation("Generation cancelled to run the command.");
    }

    if user_prompt_clean.starts_with('/') && app.handle_slash_command(&user_prompt_clean).await {
        // Slash commands never expand paste tags; release the buffers.
        app.pastes.clear();
        app.images.clear();
        if user_prompt_clean.starts_with("/fim")
            && app.session.messages.last().is_some_and(|m| m.role == "user")
        {
            app.scroll_to_bottom();
            app.start_stream_turn(event_tx.clone());
        }
        return Flow::Handled;
    }

    if user_prompt_clean.is_empty() {
        app.pastes.clear();
        app.images.clear();
        return Flow::Handled;
    }

    expand_paste_tags(app, &mut user_prompt_clean);
    app.pastes.clear();

    if app.is_streaming {
        if send_now {
            app.steer_messages.push(user_prompt_clean);
        } else {
            app.message_queue.push_back(user_prompt_clean);
        }
        app.scroll_to_bottom();
        return Flow::Handled;
    }

    corex_core::reset_repeat_guard();
    let message = app.user_message(user_prompt_clean);
    app.session.add_message(message);
    app.scroll_to_bottom();
    app.start_stream_turn(event_tx.clone());
    Flow::Handled
}

/// Removes the `$sudo:<password>` token (keeping the password in RAM) and the `$auto` / `$yolo`
/// prefix, which do not change the session mode.
fn strip_prompt_prefixes(prompt: String) -> String {
    let mut prompt = prompt;
    if let Some(pos) = prompt.find("$sudo:") {
        let after = &prompt[pos + 6..];
        let end = after.find(' ').unwrap_or(after.len());
        let pwd = &after[..end];
        if !pwd.is_empty() {
            set_sudo_password(Some(pwd.to_string()));
        }
        let before = &prompt[..pos];
        let rest = &after[end..];
        prompt = format!("{} {}", before.trim(), rest.trim()).trim().to_string();
    }
    if prompt.starts_with("$auto") || prompt.starts_with("$yolo") {
        prompt = prompt
            .trim_start_matches("$auto")
            .trim_start_matches("$yolo")
            .trim()
            .to_string();
    }
    prompt
}

/// Commands that swap or rewrite the conversation: a turn still streaming would write into it.
fn replaces_conversation(prompt: &str) -> bool {
    matches!(
        prompt.split_whitespace().next().unwrap_or(""),
        "/clear" | "/new" | "/rewind" | "/resume" | "/chat" | "/sessions" | "/fim" | "/compact" | "/compress"
    )
}

/// The prompt as history keeps it: commands that carry a secret lose their arguments.
fn history_entry(prompt: &str) -> String {
    let command = prompt.split_whitespace().next().unwrap_or("");
    if matches!(command, "/sudo" | "/key" | "/auth") {
        command.to_string()
    } else {
        prompt.to_string()
    }
}

/// Puts the pasted text back in place of its `[Pasted text #N ...]` tags, without re-scanning
/// what was inserted (a paste that contains a tag must not expand recursively).
fn expand_paste_tags(app: &App, prompt: &mut String) {
    for (id, pasted_content) in &app.pastes {
        // Trailing space anchors the id so `#1` never matches `#10`, `#11`, ...
        let prefix = format!("[Pasted text #{} ", id);
        let mut search_from = 0;
        while let Some(rel_idx) = prompt[search_from..].find(&prefix) {
            let start_idx = search_from + rel_idx;
            let Some(end_bracket) = prompt[start_idx..].find(']') else {
                break;
            };
            prompt.replace_range(start_idx..start_idx + end_bracket + 1, pasted_content);
            search_from = start_idx + pasted_content.len();
        }
    }
}

/// Empties the prompt together with its pastes and images.
fn clear_prompt(app: &mut App) {
    app.input_buffer.clear();
    app.cursor_idx = 0;
    app.pastes.clear();
    app.images.clear();
    app.slash_selected_idx = 0;
    app.last_esc_press = None;
}

/// A bare Up/Down that is really a Termux finger swipe and must scroll the chat.
fn arrow_is_swipe(app: &mut App) -> bool {
    app.tap_keyboard.in_tap_window(Instant::now())
        || (arrows_may_be_swipes(app.mouse_capture)
            && app.touch_arrows.is_swipe(Instant::now(), |d| event::poll(d).unwrap_or(false)))
}

/// Moves the slash menu selection up, wrapping to the last of `count` commands.
fn select_prev_command(app: &mut App, count: usize) {
    app.slash_selected_idx = if app.slash_selected_idx > 0 {
        app.slash_selected_idx - 1
    } else {
        count.saturating_sub(1)
    };
}

/// Recalls the previous prompt, saving the draft when history browsing starts.
fn history_prev(app: &mut App) {
    if app.input_history.is_empty() {
        return;
    }
    let idx = match app.history_idx {
        None => {
            app.saved_draft = app.input_buffer.clone();
            app.input_history.len() - 1
        }
        Some(0) => return,
        Some(i) => i - 1,
    };
    show_history_entry(app, idx);
}

/// Walks history forward; past the newest entry the saved draft comes back.
fn history_next(app: &mut App) {
    let Some(i) = app.history_idx else {
        return;
    };
    if i + 1 < app.input_history.len() {
        show_history_entry(app, i + 1);
    } else {
        app.history_idx = None;
        app.input_buffer = std::mem::take(&mut app.saved_draft);
        app.cursor_idx = app.input_buffer.len();
    }
}

fn show_history_entry(app: &mut App, idx: usize) {
    app.history_idx = Some(idx);
    app.input_buffer = app.input_history[idx].clone();
    app.cursor_idx = app.input_buffer.len();
}

/// Byte index where the word ending at the end of `before` starts (spaces before the cursor are
/// skipped), as Ctrl+Left and Ctrl+W use it.
fn word_start(before: &str) -> usize {
    before.trim_end().rfind(' ').map_or(0, |pos| pos + 1)
}

/// Byte index of the last character of `before`.
fn prev_char_start(before: &str) -> usize {
    before.char_indices().last().map_or(0, |(idx, _)| idx)
}

/// Byte length of the first character of `after`.
fn next_char_len(after: &str) -> usize {
    after.chars().next().map_or(0, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_prefixes_are_removed() {
        assert_eq!(strip_prompt_prefixes("$auto fix the build".to_string()), "fix the build");
        assert_eq!(strip_prompt_prefixes("$yolo  run it".to_string()), "run it");
        assert_eq!(strip_prompt_prefixes("plain text".to_string()), "plain text");
    }

    #[test]
    fn secrets_stay_out_of_history() {
        assert_eq!(history_entry("/sudo hunter2"), "/sudo");
        assert_eq!(history_entry("/key sk-abc123"), "/key");
        assert_eq!(history_entry("/auth sk-abc123"), "/auth");
        assert_eq!(history_entry("/model deepseek-v4-pro"), "/model deepseek-v4-pro");
        assert_eq!(history_entry("fix the build"), "fix the build");
    }

    #[test]
    fn word_and_char_boundaries() {
        assert_eq!(word_start("git commit "), 4);
        assert_eq!(word_start("single"), 0);
        assert_eq!(prev_char_start("añ"), 1);
        assert_eq!(next_char_len("ñx"), 2);
        assert_eq!(next_char_len(""), 0);
    }
}
