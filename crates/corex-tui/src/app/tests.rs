use super::*;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::layout::Rect;
use crossterm::event::{KeyCode, KeyModifiers, MouseEvent};
use corex_core::types::ToolCall;

    #[test]
    fn test_selection_ordering_is_direction_agnostic() {
        let forward = Selection { anchor: (1, 2), cursor: (3, 4) };
        let backward = Selection { anchor: (3, 4), cursor: (1, 2) };
        assert_eq!(forward.ordered(), ((1, 2), (3, 4)));
        assert_eq!(backward.ordered(), ((1, 2), (3, 4)));
        assert!(!forward.is_empty());
        assert!(Selection { anchor: (1, 1), cursor: (1, 1) }.is_empty());
    }

    #[test]
    fn test_char_range_handles_wide_glyphs_and_clamping() {
        // ASCII columns map 1:1 onto characters.
        assert_eq!(char_range_for_display_columns("hello", 0, 1), (0, 2));
        assert_eq!(char_range_for_display_columns("hello", 2, 2), (2, 3));
        // A double-width glyph covers two cells and is taken whole either way.
        assert_eq!(char_range_for_display_columns("日本", 0, 0), (0, 1));
        assert_eq!(char_range_for_display_columns("日本", 2, 3), (1, 2));
        assert_eq!(char_range_for_display_columns("日本", 0, 3), (0, 2));
        // Columns past the end of the line clamp instead of panicking.
        assert_eq!(char_range_for_display_columns("hi", 0, 99), (0, 2));
        assert_eq!(char_range_for_display_columns("", 0, 5), (0, 0));
    }

    #[tokio::test]
    async fn test_selection_text_spans_multiple_rendered_lines() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 40, 10));
        app.selection_lines = vec![
            "first line".to_string(),
            "second line".to_string(),
            "third".to_string(),
        ];
        app.selection = Some(Selection { anchor: (0, 0), cursor: (0, 4) });
        assert_eq!(app.selection_text(), "first");
        // Dragging backwards selects exactly the same text.
        app.selection = Some(Selection { anchor: (0, 4), cursor: (0, 0) });
        assert_eq!(app.selection_text(), "first");
        // Multi-line selections join the covered lines with newlines.
        app.selection = Some(Selection { anchor: (0, 6), cursor: (2, 2) });
        assert_eq!(app.selection_text(), "line\nsecond line\nthi");
    }

    #[tokio::test]
    async fn test_chat_point_maps_screen_cells_through_scroll() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(2, 5, 30, 10));
        app.scroll_offset = 100;
        // A cell inside the viewport maps onto its absolute rendered line.
        assert_eq!(app.chat_point(4, 7), Some((102, 2)));
        // Cells outside it clamp to the nearest edge instead of escaping the viewport.
        assert_eq!(app.chat_point(0, 0), Some((100, 0)));
        assert_eq!(app.chat_point(99, 99), Some((109, 29)));
    }

    #[tokio::test]
    async fn test_dragging_past_the_top_edge_scrolls_the_feed() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 2, 40, 8));
        app.total_rendered_items = 200;
        app.auto_scroll = false;
        app.scroll_offset = 100;
        app.begin_selection(0, 5);
        assert_eq!(app.selection.map(|selection| selection.anchor), Some((103, 0)));
        // Pulling above the viewport scrolls up and keeps the selection anchored.
        app.extend_selection(0, 1);
        assert_eq!(app.scroll_offset, 99);
        assert_eq!(app.selection.map(|selection| selection.cursor), Some((99, 0)));
    }

    #[tokio::test]
    async fn test_clicking_outside_the_feed_clears_the_selection() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 40, 10));
        app.begin_selection(3, 3);
        assert!(app.selection.is_some());
        // The composer sits below the feed.
        app.begin_selection(3, 20);
        assert!(app.selection.is_none());
        // A press with no drag carries no text, so releasing it copies nothing and still clears.
        app.begin_selection(1, 1);
        app.commit_selection();
        assert!(app.selection.is_none());
    }

    #[tokio::test]
    async fn test_copying_stays_out_of_the_chat_feed() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 40, 10));
        app.selection_lines = vec!["        ".to_string()];

        // Selecting blank space carries no text: no feed message, no clipboard call.
        app.selection = Some(Selection { anchor: (0, 0), cursor: (0, 7) });
        let before = app.session.messages.len();
        app.commit_selection();
        assert_eq!(app.session.messages.len(), before);
        // Releasing the drag drops the highlight too: the app owns the mouse, so it owns this.
        assert!(app.selection.is_none());
        assert!(app.selection_lines.is_empty());
    }

    /// Builds the mouse events the drag tests below feed to the app.
    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    #[tokio::test]
    async fn test_a_drag_held_above_the_feed_keeps_climbing_the_conversation() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 2, 40, 8));
        app.total_rendered_items = 500;
        app.auto_scroll = false;
        app.scroll_offset = 300;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 1, 1));
        assert_eq!(app.scroll_offset, 299, "the drag itself already moves a line");

        // Holding the pointer above the feed keeps scrolling on every tick: drag events stop as
        // soon as the mouse stops moving, so this is what lets a selection reach far-off text.
        for _ in 0..50 {
            assert!(app.autoscroll_selection());
        }
        assert_eq!(app.scroll_offset, 149);
        // The endpoint stays glued to the line under the pointer, which never moved.
        assert_eq!(app.selection.unwrap().cursor, app.chat_point(1, 1).unwrap());

        // Releasing the button ends it: nothing scrolls on its own any more.
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));
        assert!(!app.autoscroll_selection());
        assert!(app.selection.is_none());
    }

    #[tokio::test]
    async fn test_a_drag_reaching_the_top_row_keeps_scrolling_the_feed() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        // The feed is the first area on screen, so its top row is screen row 0: the highest a
        // pointer can ever reach. Dragging there is the only way to ask for more content upwards.
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 20));
        app.total_rendered_items = 500;
        app.auto_scroll = true;
        app.scroll_offset = 480;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 10));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 0));
        assert_eq!(app.scroll_offset, 479, "the drag unfreezes the pinned feed");
        assert!(!app.auto_scroll);

        // Held on that row the feed keeps climbing on every tick.
        for _ in 0..10 {
            assert!(app.autoscroll_selection());
        }
        assert_eq!(app.scroll_offset, 449);
        assert_eq!(app.selection.unwrap().cursor, app.chat_point(5, 0).unwrap());

        // Copying on release leaves the feed exactly where the user was reading.
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 0));
        assert!(!app.autoscroll_selection());
        assert_eq!(app.scroll_offset, 449);
    }

    #[tokio::test]
    async fn test_a_drag_inside_the_viewport_never_autoscrolls() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 2, 40, 8));
        app.total_rendered_items = 500;
        app.auto_scroll = false;
        app.scroll_offset = 300;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 4, 6));
        assert!(!app.autoscroll_selection());
        assert_eq!(app.scroll_offset, 300);

        // Nor without a live drag at all.
        app.clear_selection();
        assert!(!app.autoscroll_selection());
    }

    #[tokio::test]
    async fn test_the_wheel_keeps_working_while_selecting() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 40, 10));
        app.total_rendered_items = 200;
        app.auto_scroll = false;
        app.scroll_offset = 100;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0, 8));
        assert_eq!(app.selection.unwrap().cursor.0, 108);

        // Scrolling with the button held takes the endpoint with the view, so the selection stays
        // anchored to the text instead of to the screen.
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 0, 8));
        assert_eq!(app.scroll_offset, 97);
        let selection = app.selection.unwrap();
        assert_eq!(selection.anchor.0, 105, "the anchor does not move");
        assert_eq!(selection.cursor.0, 105, "the endpoint follows the text");

        // With nothing selected the wheel is a plain scroll again.
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0, 8));
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 0, 8));
        assert_eq!(app.scroll_offset, 94);
    }

    #[tokio::test]
    async fn test_scroll_chat_clamps_and_manages_auto_scroll() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        // 100 rendered lines inside a 20-line viewport -> 80 scrollable lines.
        app.total_rendered_items = 100;
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 20));
        assert_eq!(app.max_scroll_offset(), 80);

        // Auto-scroll sits at the bottom; scrolling up disengages it.
        app.auto_scroll = true;
        app.scroll_offset = 80;
        app.scroll_chat(-3);
        assert_eq!(app.scroll_offset, 77);
        assert!(!app.auto_scroll);

        // Scrolling back down onto the last line re-engages auto-scroll.
        app.scroll_chat(3);
        assert_eq!(app.scroll_offset, 80);
        assert!(app.auto_scroll);

        // Overscrolling in both directions is clamped to the valid range.
        app.scroll_chat(-1_000);
        assert_eq!(app.scroll_offset, 0);
        assert!(!app.auto_scroll);
        app.scroll_chat(1_000);
        assert_eq!(app.scroll_offset, 80);
        assert!(app.auto_scroll);

        // Content shorter than the viewport: nothing to scroll, auto-scroll stays on.
        app.total_rendered_items = 5;
        app.auto_scroll = false;
        app.scroll_chat(-5);
        assert_eq!(app.max_scroll_offset(), 0);
        assert_eq!(app.scroll_offset, 0);
        assert!(app.auto_scroll);
    }

    #[tokio::test]
    async fn test_mouse_capture_toggle_updates_state_and_reports() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        let before = app.session.messages.len();
        app.set_mouse_capture(false);
        assert!(!app.mouse_capture, "set_mouse_capture(false) must disable reporting");
        assert_eq!(app.session.messages.len(), before + 1, "each toggle must report the new state");

        // Toggling flips the state in the opposite direction.
        app.toggle_mouse_capture();
        assert!(app.mouse_capture);
        app.toggle_mouse_capture();
        assert!(!app.mouse_capture);
    }

    #[test]
    fn test_tool_large_output_is_truncated_in_render() {
        let theme = Theme::default();
        // Create huge 100KB output on a single line
        let huge_line = "A".repeat(50_000);
        let msg = Message::tool_response("call_1".to_string(), huge_line);

        let lines = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        assert!(!lines.is_empty(), "expected at least one line rendered for tool");

        // Verify the entire output spans contain "✓" and "..." and was bounded to ~140 chars
        let all_text: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all_text.contains('✓'));
        assert!(all_text.contains("..."));
        assert!(all_text.len() < 300, "tool summary should be truncated: length was {}", all_text.len());
        assert!(lines.len() <= 3, "expected at most 3 wrapped lines, got {}", lines.len());
    }

    #[test]
    fn test_tool_multiline_output_expand() {
        let theme = Theme::default();
        let multiline_output = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8";
        let msg = Message::tool_response("call_2".to_string(), multiline_output.to_string());

        // 1. Compact mode (logs_expanded = false) -> header + omission line + last 5 lines
        let lines_compact = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        let all_compact: String = lines_compact.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all_compact.contains("Output (8 lines)"));
        assert!(all_compact.contains("… (+3 lines above)"));
        assert!(all_compact.contains("line 8"));

        // 2. Expanded mode (logs_expanded = true) -> header + all 8 lines
        let lines_expanded = render_single_message_with_pending(0, &msg, None, &theme, 80, None, true);
        let all_expanded: String = lines_expanded.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all_expanded.contains("Output (8 lines)"));
        assert!(!all_expanded.contains("lines above"));
        assert!(all_expanded.contains("line 1"));
        assert!(all_expanded.contains("line 8"));
    }

    #[test]
    fn test_edited_file_preview_paints_removals_as_removals() {
        let theme = Theme::default();
        let output = "Successfully edited src/lib.rs (line 2):\n    1 | context\n-   2 | gone\n+   2 | fresh\n    3 | tail\n";
        let msg = Message::tool_response("call_1".to_string(), output);

        let lines = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        let fg_of = |needle: &str| -> Option<Color> {
            let line = lines.iter().find(|line| {
                let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
                text.contains(needle)
            })?;
            line.spans.last().and_then(|span| span.style.fg)
        };

        assert_eq!(fg_of("gone"), Some(theme.diff_removed_fg), "a line the edit removed must read as removed");
        assert_eq!(fg_of("fresh"), Some(theme.diff_added_fg), "and a line it added as added");
        assert_eq!(fg_of("context"), Some(theme.gray), "untouched context is neither: it just dims");
    }

    #[tokio::test]
    async fn test_app_cache_invalidation() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        app.cached_message_count = 10;
        app.cached_render_width = 80;
        app.cached_session_id = "test_sess".to_string();
        app.cached_message_lines.push(Line::from("cached line"));

        app.invalidate_message_cache();

        assert_eq!(app.cached_message_count, 0);
        assert_eq!(app.cached_render_width, 0);
        assert!(app.cached_session_id.is_empty());
        assert!(app.cached_message_lines.is_empty());
    }

    #[test]
    fn test_format_input_spans_with_paste_tag() {
        let theme = Theme::default();
        let text = "Look at this: [Pasted text #1 +17 lines] and continue";
        let spans = format_input_spans(text, &theme);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "Look at this: ");
        assert_eq!(spans[1].content, "[Pasted text #1 +17 lines]");
        assert_eq!(spans[2].content, " and continue");
    }

    #[test]
    fn test_paste_expansion_logic() {
        let mut user_prompt_clean = "Explain this: [Pasted text #1 +2 lines] and fix it.".to_string();
        let mut pastes = std::collections::HashMap::new();
        // Content itself contains [Pasted text #1] to test recursion immunity!
        pastes.insert(1, "error line 1 with [Pasted text #1] inside\nerror line 2\nerror line 3".to_string());

        for (id, pasted_content) in &pastes {
            let prefix = format!("[Pasted text #{}", id);
            let mut search_from = 0;
            while let Some(rel_idx) = user_prompt_clean[search_from..].find(&prefix) {
                let start_idx = search_from + rel_idx;
                let rest = &user_prompt_clean[start_idx..];
                if let Some(end_bracket) = rest.find(']') {
                    user_prompt_clean.replace_range(start_idx..start_idx + end_bracket + 1, pasted_content);
                    search_from = start_idx + pasted_content.len();
                } else {
                    break;
                }
            }
        }

        assert_eq!(
            user_prompt_clean,
            "Explain this: error line 1 with [Pasted text #1] inside\nerror line 2\nerror line 3 and fix it."
        );
    }

    #[test]
    fn test_format_input_with_cursor_empty() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("", 0, &theme);
        assert_eq!(col, 0);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "█");
    }

    #[test]
    fn test_format_input_with_cursor_end() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("cargo check", 11, &theme);
        assert_eq!(col, 11);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "cargo check");
        assert_eq!(spans[1].content, "█");
    }

    #[test]
    fn test_format_input_with_cursor_middle() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("cargo check", 5, &theme);
        assert_eq!(col, 5);
        // Spans: "cargo" (before), " " (cursor highlighted), "check" (after)
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "cargo");
        assert_eq!(spans[1].content, " ");
        assert_eq!(spans[2].content, "check");
    }

    #[test]
    fn test_format_input_with_cursor_paste_tag() {
        let theme = Theme::default();
        let text = "run [Pasted text #1 +5 lines] now";
        let (spans, col) = format_input_with_cursor(text, 4, &theme);
        assert_eq!(col, 4);
        assert_eq!(spans[0].content, "run ");
        assert_eq!(spans[1].content, "[Pasted text #1 +5 lines]");
        assert_eq!(spans[2].content, " now");
    }

    #[tokio::test]
    async fn test_generation_id_increment() {
        let mut app = App::new(
            LlmClient::new(Config::default()),
            PathBuf::from("/tmp"),
            false,
        );
        assert_eq!(app.current_generation_id, 0);
        app.current_generation_id = app.current_generation_id.wrapping_add(1);
        assert_eq!(app.current_generation_id, 1);
    }

    #[tokio::test]
    async fn test_slash_popup_animation_targets() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();

        // 1. Empty buffer -> target is 0.0, no animation
        assert_eq!(app.slash_popup_target_height(), 0.0);
        assert!(!app.is_slash_animating());

        // 2. Buffer starts with '/' -> target is > 0, animation starts
        app.input_buffer = "/".to_string();
        assert!(app.slash_popup_target_height() >= 3.0);
        assert!(app.is_slash_animating());

        // 3. Buffer has /model -> target is 3.0 (1 item + 2 borders)
        app.input_buffer = "/model".to_string();
        assert_eq!(app.slash_popup_target_height(), 3.0);

        // 4. Modal is open -> target must be 0.0 even if buffer has '/'
        app.model_dialog.is_open = true;
        assert_eq!(app.slash_popup_target_height(), 0.0);
    }

    #[tokio::test]
    async fn test_slash_menu_disabled_while_browsing_history() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();

        // Typing an exact command name opens the completion menu...
        app.input_buffer = "/resume".to_string();
        app.history_idx = None;
        assert!(app.is_slash_menu_active());
        assert_eq!(app.matching_slash_commands(), vec!["/resume"]);
        assert_eq!(app.slash_popup_target_height(), 3.0);

        // ...but recalling that same command from history must NOT, otherwise the
        // single-item menu would swallow the arrow keys and block history browsing.
        app.history_idx = Some(3);
        assert!(!app.is_slash_menu_active());
        assert!(app.matching_slash_commands().is_empty());
        assert_eq!(app.slash_popup_target_height(), 0.0);

        // Any exact command name recalled from history is equally safe.
        for entry in ["/resume", "/chat", "/save", "/model", "/yolo"] {
            app.input_buffer = entry.to_string();

            // Freshly typed -> the menu is active (this is the state that used to trap the arrows).
            app.history_idx = None;
            assert!(app.is_slash_menu_active(), "typed '{entry}' should open the menu");

            // Recalled from history -> the menu is inert, so Up/Down keep browsing.
            app.history_idx = Some(0);
            assert!(!app.is_slash_menu_active(), "recalled '{entry}' must not capture the arrow keys");

            // Entries with arguments never match a bare command name in either mode.
            app.input_buffer = format!("{entry} 2");
            assert!(!app.is_slash_menu_active());
            app.history_idx = None;
            assert!(!app.is_slash_menu_active());
        }
    }

    #[test]
    fn test_format_system_message_model_activation() {
        let theme = Theme::default();
        let msg = "Activated DeepSeek-V4-Pro (Deep Reasoning Engine)\n- Reasoning Depth: max\n- Search CoT: low";
        let lines = format_system_message(msg, &theme, 80, false);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[model]")));
        assert!(lines[1].spans.iter().any(|s| s.content.contains("Reasoning Depth:")));
        assert!(lines[2].spans.iter().any(|s| s.content.contains("Search CoT:")));
    }

    #[test]
    fn test_find_url_in_line() {
        assert_eq!(
            find_url_in_line("Creator: sluisr (https://sluisr.com)", None),
            Some("https://sluisr.com".to_string())
        );
        assert_eq!(
            find_url_in_line("│   Official Website       https://corex.sluisr.com                          │", None),
            Some("https://corex.sluisr.com".to_string())
        );
        assert_eq!(
            find_url_in_line("Visit https://github.com/sluisr/corex/issues.", None),
            Some("https://github.com/sluisr/corex/issues".to_string())
        );
        assert_eq!(
            find_url_in_line("Active Model: deepseek-flash", None),
            None
        );
    }

    #[test]
    fn test_format_system_message_task_done() {
        let theme = Theme::default();
        
        // 1. Success task with empty output -> single badge line
        let msg = "TASK_DONE|201653|0|0.2s\n";
        let lines = format_system_message(msg, &theme, 80, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[bg:201653]")));
        assert!(lines[0].spans.iter().any(|s| s.content.contains("finished (exit 0)")));

        // 2. Failed task with > 5 output lines -> badge + omission line + last 5 lines (tail)
        let msg_err = "TASK_DONE|201654|2|0.5s\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7";
        let lines_err = format_system_message(msg_err, &theme, 80, false);
        // Line 0: badge, Line 1: … (+2 lines above, Ctrl+O to expand), Lines 2..7: last 5 lines
        assert_eq!(lines_err.len(), 7);
        assert!(lines_err[0].spans.iter().any(|s| s.content.contains("[bg:201654]")));
        assert!(lines_err[0].spans.iter().any(|s| s.content.contains("finished (exit 2)")));
        assert!(lines_err[1].spans.iter().any(|s| s.content.contains("… (+2 lines above")));
        let line2_text: String = lines_err[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line2_text.contains("line 3"));
        let line6_text: String = lines_err[6].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line6_text.contains("line 7"));

        // 3. Expanded logs -> shows all 7 lines (badge + 7 lines = 8 lines)
        let lines_expanded = format_system_message(msg_err, &theme, 80, true);
        assert_eq!(lines_expanded.len(), 8);
        let exp_line1_text: String = lines_expanded[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(exp_line1_text.contains("line 1"));
    }

    #[tokio::test]
    async fn test_balance_slash_command_non_blocking() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        assert!(!app.is_checking_balance);
        let handled = app.handle_slash_command("/balance").await;
        assert!(handled);
        assert!(app.is_checking_balance);
        assert_eq!(app.active_status.as_deref(), Some("Checking account balance..."));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].text_content(), Some("Checking account balance..."));
    }

    #[tokio::test]
    async fn test_paste_while_streaming() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        app.is_streaming = true;

        app.handle_paste("hello world".to_string());
        assert_eq!(app.input_buffer, "hello world");
        assert_eq!(app.cursor_idx, 11);

        // Multiline paste while streaming creates paste tag
        app.handle_paste("\nsecond line\nthird line".to_string());
        assert!(app.input_buffer.contains("[Pasted text #1 +2 lines]"));
        assert_eq!(app.pastes.len(), 1);
    }

    #[test]
    fn test_user_message_box_rendering() {
        let theme = Theme::default();
        let msg = Message::user("hola mundo".to_string());
        let lines = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);

        let rendered_text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();

        // 1. Must NOT contain the old "❯ " glyph
        for line in &rendered_text {
            assert!(!line.contains('❯'), "User message should not contain ❯: {}", line);
        }

        // 2. Clean typography: starts with "You: " and the user message
        assert!(rendered_text[0].contains("You:"));
        assert!(rendered_text[0].contains("hola mundo"));
    }

    #[tokio::test]
    async fn test_message_queuing_while_streaming() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.auth_dialog.close();
        app.is_streaming = true;

        // Queue two messages
        app.message_queue.push_back("First queued task".to_string());
        app.message_queue.push_back("Second queued task".to_string());
        assert_eq!(app.message_queue.len(), 2);

        // Dequeue first message
        let first = app.message_queue.pop_front().unwrap();
        assert_eq!(first, "First queued task");
        app.session.add_message(Message::user(first));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].text_content(), Some("First queued task"));

        // Dequeue second message
        let second = app.message_queue.pop_front().unwrap();
        assert_eq!(second, "Second queued task");
        app.session.add_message(Message::user(second));
        assert_eq!(app.session.messages.len(), 2);
        assert_eq!(app.session.messages[1].text_content(), Some("Second queued task"));
        assert!(app.message_queue.is_empty());
    }

    #[test]
    fn test_ctrl_o_key_variants() {
        let is_ctrl_o = |key: crossterm::event::KeyEvent| -> bool {
            let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            (is_ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')))
                || key.code == KeyCode::Char('\x0f')
        };

        // 1. Standard lowercase Ctrl+o
        let key1 = crossterm::event::KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert!(is_ctrl_o(key1));

        // 2. Uppercase Ctrl+O (Shift or CapsLock)
        let key2 = crossterm::event::KeyEvent::new(KeyCode::Char('O'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert!(is_ctrl_o(key2));

        // 3. Raw ASCII 0x0F (sent by legacy/tmux terminal modes without CSI u)
        let key3 = crossterm::event::KeyEvent::new(KeyCode::Char('\x0f'), KeyModifiers::NONE);
        assert!(is_ctrl_o(key3));

        let key4 = crossterm::event::KeyEvent::new(KeyCode::Char('\x0f'), KeyModifiers::CONTROL);
        assert!(is_ctrl_o(key4));

        // 4. False positives check
        let normal_o = crossterm::event::KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        assert!(!is_ctrl_o(normal_o));

        let ctrl_p = crossterm::event::KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(!is_ctrl_o(ctrl_p));
    }

    #[tokio::test]
    async fn test_session_allowed_commands_does_not_activate_yolo() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        assert!(!app.always_allow_tools);
        assert!(app.session_allowed_commands.is_empty());

        let call = ToolCall::new("call_1", "run_shell_command", serde_json::json!({ "command": "cargo test --workspace" }).to_string());

        let rule = extract_tool_allow_rule(&call);
        assert_eq!(rule, "cargo test");

        // Simulate choosing option 2 ("Allow for this session")
        app.session_allowed_commands.push(rule.clone());

        // YOLO mode must still be FALSE!
        assert!(!app.always_allow_tools);

        // Effective allowed commands must include "cargo test"
        let eff = app.get_effective_allowed_commands();
        assert!(eff.contains(&"cargo test".to_string()));
    }
