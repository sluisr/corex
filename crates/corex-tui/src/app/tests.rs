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
        app.selection_lines.tail = vec![
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
        app.selection_lines.tail = vec!["        ".to_string()];

        // Selecting blank space carries no text: no feed message, no clipboard call.
        app.selection = Some(Selection { anchor: (0, 0), cursor: (0, 7) });
        let before = app.session.messages.len();
        app.commit_selection();
        assert_eq!(app.session.messages.len(), before);
        // Releasing the drag drops the highlight too: the app owns the mouse, so it owns this.
        assert!(app.selection.is_none());
    }

    #[tokio::test]
    async fn test_mouse_click_url_opens_link_deliberately() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
        app.selection_lines.tail = vec!["  │ 🌐 Web personal │ https://sluisr.com               │".to_string()];
        // Simulate app running for more than 1 second (past startup grace)
        app.app_start_time = Instant::now() - Duration::from_secs(5);

        // Click directly on the URL at column 25 with 100ms realistic click tap
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(100));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });

        assert!(app.selection.is_none());
        assert_eq!(app.last_url_open.as_ref().map(|(_, u)| u.as_str()), Some("https://sluisr.com"));

        // Rapid duplicate click within 1.5s debounce is blocked
        let first_open_time = app.last_url_open.as_ref().unwrap().0;
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(100));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });
        assert_eq!(app.last_url_open.as_ref().unwrap().0, first_open_time);
    }

    #[tokio::test]
    async fn test_window_focus_activation_click_is_strictly_ignored() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
        app.selection_lines.tail = vec!["  │ 🌐 Web personal │ https://sluisr.com               │".to_string()];
        app.app_start_time = Instant::now() - Duration::from_secs(5);

        // Window activation click (FocusGained arrived 50ms ago)
        app.last_focus_gained = Some(Instant::now() - Duration::from_millis(50));

        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(50));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });

        // Must NOT open any URL!
        assert!(app.last_url_open.is_none());
    }

    #[tokio::test]
    async fn test_startup_grace_period_ignores_clicks() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
        app.selection_lines.tail = vec!["  │ 🌐 Web personal │ https://sluisr.com               │".to_string()];
        // App just started 100ms ago (< 800ms threshold)
        app.app_start_time = Instant::now() - Duration::from_millis(100);

        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(60));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 25, row: 0, modifiers: KeyModifiers::NONE });

        assert!(app.last_url_open.is_none());
    }

    #[tokio::test]
    async fn test_drag_selection_does_not_open_link() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
        app.selection_lines.tail = vec!["  │ 🌐 Web personal │ https://sluisr.com               │".to_string()];
        app.app_start_time = Instant::now() - Duration::from_secs(5);

        // Click and drag across text (from col 22 to 40, elapsed 600ms)
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 22, row: 0, modifiers: KeyModifiers::NONE });
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), column: 40, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(600));

        app.inside_tmux = false;
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 40, row: 0, modifiers: KeyModifiers::NONE });

        // Must NOT open link because it's a drag selection! In a plain terminal the copied text
        // stays highlighted; under tmux it is released like tmux's own selection.
        assert!(app.last_url_open.is_none());
        assert!(app.selection.is_some());
        assert!(app.drag_point.is_none());
        // Leaving the window (focus follows the mouse) keeps a finished selection.
        app.cancel_drag();
        assert!(app.selection.is_some());

        app.inside_tmux = true;
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 22, row: 0, modifiers: KeyModifiers::NONE });
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), column: 40, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(600));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 40, row: 0, modifiers: KeyModifiers::NONE });
        assert!(app.selection.is_none());
    }

    #[tokio::test]
    async fn test_click_on_non_url_text_does_not_open_link() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
        app.selection_lines.tail = vec!["  │ 🌐 Web personal │ https://sluisr.com               │".to_string()];
        app.app_start_time = Instant::now() - Duration::from_secs(5);

        // Click on "Web personal" (col 5) instead of URL
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row: 0, modifiers: KeyModifiers::NONE });
        app.mouse_down_time = Some(Instant::now() - Duration::from_millis(100));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: 5, row: 0, modifiers: KeyModifiers::NONE });

        assert!(app.last_url_open.is_none());
    }

    #[tokio::test]
    async fn test_click_outside_chat_clears_selection() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 2, 80, 10));
        app.selection = Some(Selection { anchor: (1, 5), cursor: (1, 15) });

        // Click outside chat (row 0 is above last_chat_rect)
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 10, row: 0, modifiers: KeyModifiers::NONE });
        assert!(app.selection.is_none());
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
    async fn test_scroll_to_bottom_and_smart_lock() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        app.total_rendered_items = 100;
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 20));
        app.scroll_offset = 80;
        app.auto_scroll = true;

        // Scroll up to read
        app.scroll_chat(-20);
        assert_eq!(app.scroll_offset, 60);
        assert!(!app.auto_scroll);

        // scroll_to_bottom snaps back to 80 and enables auto_scroll
        app.scroll_to_bottom();
        assert_eq!(app.scroll_offset, 80);
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

    #[tokio::test]
    async fn only_the_focused_field_shows_a_caret() {
        let theme = Theme::default();
        let mut app = App::new(LlmClient::new(Config::default()), PathBuf::from("/tmp"), false);
        assert!(!app.dialog_has_focus());
        app.sudo_dialog.open(ToolCall::new("c", "run_shell_command", r#"{"command":"sudo id"}"#), "sudo id".into());
        assert!(app.dialog_has_focus());

        let (spans, col) = format_input_with_cursor("cargo check", 5, &theme, false);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "cargo check", "no caret glyph and no highlighted character");
        assert!(spans.iter().all(|s| s.style.bg.is_none() || s.style.bg == Some(Color::Reset)));
        assert_eq!(col, 5);
    }

    #[test]
    fn test_format_input_with_cursor_empty() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("", 0, &theme, true);
        assert_eq!(col, 0);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "█");
    }

    #[test]
    fn test_format_input_with_cursor_end() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("cargo check", 11, &theme, true);
        assert_eq!(col, 11);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "cargo check");
        assert_eq!(spans[1].content, "█");
    }

    #[test]
    fn test_format_input_with_cursor_middle() {
        let theme = Theme::default();
        let (spans, col) = format_input_with_cursor("cargo check", 5, &theme, true);
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
        let (spans, col) = format_input_with_cursor(text, 4, &theme, true);
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
        let msg = "Activated DeepSeek-V4-Pro (Deep Reasoning Engine)\n- Reasoning Depth: max\n- Temperature: 1.0";
        let lines = format_system_message(msg, &theme, 80, false);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[model]")));
        assert!(lines[1].spans.iter().any(|s| s.content.contains("Reasoning Depth:")));
        assert!(lines[2].spans.iter().any(|s| s.content.contains("Temperature:")));
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

        // Click column tests: clicking outside the URL must NEVER open it!
        let table_row = "  │ 🌐 Web personal │ https://sluisr.com               │";
        // Clicking on "Web personal" (col 5) -> None!
        assert_eq!(find_url_in_line(table_row, Some(5)), None);
        // Clicking on table divider (col 18) -> None!
        assert_eq!(find_url_in_line(table_row, Some(18)), None);
        // Clicking directly on "https://sluisr.com" (col 25) -> Some!
        assert_eq!(find_url_in_line(table_row, Some(25)), Some("https://sluisr.com".to_string()));

        // Markdown link [label](url) tests:
        let md_line = "Visit [my portfolio](https://sluisr.com) to view projects";
        // Clicking on "my portfolio" label (col 10) -> Some!
        assert_eq!(find_url_in_line(md_line, Some(10)), Some("https://sluisr.com".to_string()));
        // Clicking directly on "https://sluisr.com" (col 25) -> Some!
        assert_eq!(find_url_in_line(md_line, Some(25)), Some("https://sluisr.com".to_string()));
        // Clicking outside on "Visit" (col 2) or "view" (col 44) -> None!
        assert_eq!(find_url_in_line(md_line, Some(2)), None);
        assert_eq!(find_url_in_line(md_line, Some(44)), None);
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

    #[test]
    fn test_format_system_message_subagent_done() {
        let theme = Theme::default();

        // 1. Success subagent (Scout) with empty output -> single badge line
        let msg = "SUBAGENT_DONE|sub_c7ac96e4|SCOUT|1.2s\n";
        let lines = format_system_message(msg, &theme, 80, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("✓")));
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[scout:sub_c7ac96e4]")));
        assert!(lines[0].spans.iter().any(|s| s.content.contains("completed")));
        assert!(lines[0].spans.iter().any(|s| s.content.contains("1.2s")));

        // 2. Subagent with many output lines -> badge + omission line + 5 lines preview
        let msg_long = "SUBAGENT_DONE|sub_1234|TESTER|3s\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8";
        let lines_long = format_system_message(msg_long, &theme, 80, false);
        assert_eq!(lines_long.len(), 7); // 1 badge + 1 omission + 5 lines preview
        assert!(lines_long[0].spans.iter().any(|s| s.content.contains("✓")));
        assert!(lines_long[0].spans.iter().any(|s| s.content.contains("[tester:sub_1234]")));
        assert!(lines_long[1].spans.iter().any(|s| s.content.contains("… (+3 lines above")));

        // 3. Subagent with failed exit
        let msg_fail = "SUBAGENT_DONE|sub_err1|WORKER|0.1s\nExit code 1:\nFatal compile error";
        let lines_fail = format_system_message(msg_fail, &theme, 80, false);
        assert!(lines_fail[0].spans.iter().any(|s| s.content.contains("failed")));
        assert!(lines_fail[0].spans.iter().any(|s| s.content.contains("✕")));
        assert!(lines_fail[0].spans.iter().any(|s| s.content.contains("[worker:sub_err1]")));

        // 4. Expanded logs (Ctrl+O)
        let lines_expanded = format_system_message(msg_long, &theme, 80, true);
        assert_eq!(lines_expanded.len(), 9); // 1 badge + 8 output lines

        // 5. Without underscore (SUBAGENTDONE|)
        let msg_no_us = "SUBAGENTDONE|sub33a3f099|SCOUT|0s\n";
        let lines_no_us = format_system_message(msg_no_us, &theme, 80, false);
        assert_eq!(lines_no_us.len(), 1);
        assert!(lines_no_us[0].spans.iter().all(|s| !s.content.contains("🔭")));
        assert!(lines_no_us[0].spans.iter().any(|s| s.content.contains("[scout:sub33a3f099]")));
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

        // 2. Clean typography: starts with "[You]" and the user message
        assert!(rendered_text[0].contains("[You]"));
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

    #[test]
    fn test_extract_tool_allow_rules_unifies_file_edits() {
        let patch_call = ToolCall::new("c1", "apply_patch", r#"{"patch":"diff..."}"#.to_string());
        let rules = extract_tool_allow_rules(&patch_call);
        assert!(rules.contains(&"apply_patch".to_string()));
        assert!(rules.contains(&"edit".to_string()));
        assert!(rules.contains(&"write_file".to_string()));

        let edit_call = ToolCall::new("c2", "edit", r#"{"file_path":"test.rs","old_string":"a","new_string":"b"}"#.to_string());
        let rules = extract_tool_allow_rules(&edit_call);
        assert!(rules.contains(&"apply_patch".to_string()));
        assert!(rules.contains(&"edit".to_string()));
        assert!(rules.contains(&"write_file".to_string()));
    }

    #[test]
    fn test_skill_tool_output_suppresses_logs() {
        let theme = Theme::default();
        let mut msg = Message::tool_response("call_1", "---\nname: corex-about\n---\n# Full Skill Markdown\nLots of text");
        msg.name = Some("activate_skill".to_string());

        let lines = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(!joined.contains("Full Skill Markdown"), "Skill tool output must not dump logs in UI");
        assert!(!joined.contains("Lots of text"), "Skill tool output must not dump logs in UI");
    }

    #[test]
    fn test_format_tool_call_spans_no_emojis() {
        let theme = Theme::default();
        let spans = format_tool_call_spans("run_shell_command", r#"{"command":"cargo build"}"#, &theme);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!joined.contains('⚡'), "Must not contain lightning bolt emoji: {}", joined);
        assert!(!joined.contains('›'), "Must not contain chevron: {}", joined);
        assert_eq!(joined, " $ cargo build ");
        assert!(spans.iter().all(|s| s.style.bg == Some(theme.code_bg)), "the command sits on a shaded chip");

        let search_spans = format_tool_call_spans("web_search", r#"{"query":"sluisr sluisr.com Linux"}"#, &theme);
        let search_joined: String = search_spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(search_joined, "web \"sluisr sluisr.com Linux\"");

        let skill_spans = format_tool_call_spans("activate_skill", r#"{"name":"corex-about"}"#, &theme);
        let skill_joined: String = skill_spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!skill_joined.contains('⚙'), "Must not contain gear emoji: {}", skill_joined);
        assert_eq!(skill_joined, "skill corex-about");

        let read_spans = format_tool_call_spans("read_file", r#"{"file_path":"src/main.rs"}"#, &theme);
        let read_joined: String = read_spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(read_joined, "read src/main.rs");

        let list_spans = format_tool_call_spans("list_directory", r#"{"path":"crates"}"#, &theme);
        let list_joined: String = list_spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(list_joined, "ls crates/");

        let agent_spans = format_tool_call_spans("delegate_subagent", r#"{"role":"scout","prompt":"inspect codebase"}"#, &theme);
        let agent_joined: String = agent_spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(agent_joined, "agent scout inspect codebase");

        let agent_task_spans = format_tool_call_spans("delegate_subagent", r#"{"role":"tester","task":"run all tests"}"#, &theme);
        let agent_task_joined: String = agent_task_spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(agent_task_joined, "agent tester run all tests");
    }

    #[tokio::test]
    async fn test_scroll_to_bottom_preserves_input_draft() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.total_rendered_items = 100;
        app.scroll_offset = 10;
        app.auto_scroll = false;
        app.input_buffer = "my drafted prompt text".to_string();
        app.cursor_idx = app.input_buffer.len();

        // scroll_to_bottom snaps to bottom and sets auto_scroll true
        app.scroll_to_bottom();
        assert!(app.auto_scroll);
        assert_eq!(app.input_buffer, "my drafted prompt text");
    }

    #[tokio::test]
    async fn test_esc_clears_prompt_only_when_double_tapped_and_preserves_on_scroll() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.input_buffer = "keep this text".to_string();
        app.cursor_idx = app.input_buffer.len();
        app.auto_scroll = false;

        // When scrolled up, scroll_to_bottom restores auto_scroll and leaves text intact
        if !app.auto_scroll {
            app.scroll_to_bottom();
            app.last_esc_press = None;
        }
        assert!(app.auto_scroll);
        assert_eq!(app.input_buffer, "keep this text");
        assert_eq!(app.last_esc_press, None);

        // When at bottom, 1st Esc sets last_esc_press
        app.last_esc_press = Some(Instant::now());
        assert_eq!(app.input_buffer, "keep this text");

        // 2nd Esc within 500ms clears prompt
        let is_recent = app.last_esc_press
            .map(|i| i.elapsed() <= Duration::from_millis(500))
            .unwrap_or(false);
        assert!(is_recent);
        if is_recent {
            app.input_buffer.clear();
            app.cursor_idx = 0;
            app.last_esc_press = None;
        }
        assert!(app.input_buffer.is_empty());
        assert_eq!(app.cursor_idx, 0);
    }

    #[test]
    fn test_grep_pattern_extraction_never_renders_empty_quotes() {
        use crate::app::formatting::format_tool_call_spans;
        let theme = Theme::default();
        
        // 1. Tool call using "pattern" argument (the schema canonical parameter)
        let spans = format_tool_call_spans("grep_search", r#"{"pattern":"my_struct","dir_path":"src"}"#, &theme);
        let joined: String = spans.into_iter().map(|s| s.content.to_string()).collect();
        assert!(joined.contains("\"my_struct\""), "Expected pattern to be extracted, got: {}", joined);
        assert!(!joined.contains("\"\""), "Must never render empty quotes, got: {}", joined);

        // 2. Tool call using "query" fallback
        let spans_q = format_tool_call_spans("grep", r#"{"query":"test_fn","path":"crates"}"#, &theme);
        let joined_q: String = spans_q.into_iter().map(|s| s.content.to_string()).collect();
        assert!(joined_q.contains("\"test_fn\""), "Expected query to be extracted, got: {}", joined_q);
        assert!(!joined_q.contains("\"\""), "Must never render empty quotes, got: {}", joined_q);
    }

    #[test]
    fn test_format_call_preview_utf8_boundary_no_panic() {
        use crate::app::event_loop::format_call_preview;
        use corex_core::types::{FunctionCall, ToolCall};
        use corex_tools::ToolRegistry;

        let registry = ToolRegistry::new();
        // 59 ASCII bytes followed by 'ó' (2 bytes: 59..61) so byte index 60 falls inside 'ó'
        let val = format!("{}ó_extra_text", "a".repeat(59));
        let call = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "custom_tool".to_string(),
                arguments: serde_json::json!({ "query": val }).to_string(),
            },
            extra_content: None,
        };
        let preview = format_call_preview(&call, &registry, std::path::Path::new("."));
        assert!(preview.contains("..."));
    }

    #[tokio::test]
    async fn test_fleet_mode_session_isolation() {
        use corex_core::types::Message;
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);

        // Populate normal conversation session
        app.session.add_message(Message::user("Auditing Linux kernel staging drivers"));
        let normal_id = app.session.id.clone();
        assert_eq!(app.session.messages.len(), 1);

        // Enter fleet mode
        app.enter_fleet_mode();
        assert!(app.normal_session_stash.is_some());
        assert_ne!(app.session.id, normal_id);
        assert_eq!(app.session.tag.as_deref(), Some("fleet"));
        assert_eq!(app.session.title, "SysAdmin Fleet Console");

        // Add a sysadmin command to the fleet session
        app.session.add_message(Message::user("fleet_run_command df -h"));
        let fleet_id = app.session.id.clone();

        // Exit fleet mode -> normal session must be restored cleanly
        app.exit_fleet_mode();
        assert!(app.normal_session_stash.is_none());
        assert_eq!(app.session.id, normal_id);
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].text_content(), Some("Auditing Linux kernel staging drivers"));

        // Fleet session must be stashed in fleet_session
        assert!(app.fleet_session.is_some());
        let stashed_fleet = app.fleet_session.as_ref().unwrap();
        assert_eq!(stashed_fleet.id, fleet_id);

        // Re-entering fleet mode restores the fleet session without creating a new blank one
        app.enter_fleet_mode();
        assert_eq!(app.session.id, fleet_id);
        assert!(app.session.messages.iter().any(|m| m.text_content() == Some("fleet_run_command df -h")));

        // Exit back to normal
        app.exit_fleet_mode();
        assert_eq!(app.session.id, normal_id);
    }

    #[test]
    fn test_touch_mobile_detection_and_history_shortcuts() {
        use crate::app::mobile::is_mobile_touch_environment;

        // Desktop standard dimensions (e.g. 120x40) without COREX_MOBILE env must be false on desktop
        std::env::remove_var("COREX_MOBILE");
        if !crate::app::mobile::is_android_or_termux() {
            assert!(!is_mobile_touch_environment(120, 40));
        }

        // Forced COREX_MOBILE=1 must return true regardless of dimensions
        std::env::set_var("COREX_MOBILE", "1");
        assert!(is_mobile_touch_environment(120, 40));
        assert!(is_mobile_touch_environment(50, 30));
        std::env::remove_var("COREX_MOBILE");
    }

    #[test]
    fn test_touch_arrow_classifier_separates_swipes_from_key_presses() {
        use crate::app::mobile::TouchArrowClassifier;
        use std::time::{Duration, Instant};

        std::env::remove_var("COREX_TOUCH_SCROLL_MS");
        let mut arrows = TouchArrowClassifier::default();
        let t0 = Instant::now();

        // A lone arrow with nothing behind it is a key press: history navigation.
        assert!(!arrows.is_swipe(t0, |_| false));
        // An arrow followed at once by more input is the start of a swipe burst.
        assert!(arrows.is_swipe(t0, |window| window == Duration::from_millis(50)));
        // Mid-swipe arrows keep scrolling without waiting, even after a slow stretch.
        let mid = t0 + Duration::from_millis(200);
        assert!(arrows.is_swipe(mid, |_| panic!("must not wait during a swipe")));
        // Once the finger has been still long enough, the next lone arrow is a key again.
        assert!(!arrows.is_swipe(mid + Duration::from_millis(400), |_| false));

        // COREX_TOUCH_SCROLL_MS=0 turns detection off: every arrow is a key press.
        std::env::set_var("COREX_TOUCH_SCROLL_MS", "0");
        assert!(!arrows.is_swipe(t0, |_| true));
        std::env::remove_var("COREX_TOUCH_SCROLL_MS");
    }

    #[test]
    fn test_arrows_are_never_swipes_with_mouse_reporting() {
        use crate::app::mobile::arrows_may_be_swipes;

        // With mouse reporting on, wheel and swipes are mouse events, so arrows are keys. Off, only
        // Termux turns a swipe into arrows; a desktop arrow is always a key.
        assert!(!arrows_may_be_swipes(true));
        assert_eq!(arrows_may_be_swipes(false), crate::app::mobile::is_android_or_termux());
    }

    #[tokio::test]
    async fn test_fleet_mode_exit_warning_not_truncated() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use crate::app::render::render_ui;

        let mut app = App::new(
            corex_core::client::LlmClient::new(corex_core::config::Config::default()),
            std::path::PathBuf::from("/tmp"),
            false,
        );
        app.git_branch = "main".to_string();
        app.auth_dialog.close();
        app.enter_fleet_mode();

        // Simulate Ctrl+C press
        app.last_ctrl_c_press = Some(std::time::Instant::now());

        // In fleet split mode on a 130x30 terminal (chat area = ~71 cols)
        let backend = TestBackend::new(130, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app)).unwrap();

        // Inspect the status footer row (row 29)
        let buffer = terminal.backend().buffer();
        let footer_line: String = (0..70)
            .map(|x| buffer[(x, 29)].symbol())
            .collect();

        // The warning must NOT be cut off at "Press Ctrl+C ag"
        assert!(
            footer_line.contains("Press Ctrl+C again to exit"),
            "Footer in fleet mode truncated the exit warning! Found: '{}'",
            footer_line
        );
    }

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn test_about_card_rows_stay_inside_the_card_when_narrow() {
        let theme = Theme::default();
        for width in [20, 50, 60, 80, 120] {
            let lines = render_about_card(
                "0.5.0", "deepseek-flash", "https://api.deepseek.com", "qwen", true,
                "sess_123", "/home/user/projects/a-rather-long-workspace-name", "main", &theme, width,
            );
            let limit = width.saturating_sub(6).clamp(44, 72) + 4;
            for row in plain(&lines) {
                let w = unicode_width::UnicodeWidthStr::width(row.as_str());
                assert!(w <= limit, "width {}: row {:?} is {} wide (limit {})", width, row, w, limit);
                assert!(!row.ends_with('│') || row.trim() == "│", "width {}: closed right border in {:?}", width, row);
            }
        }
    }

    #[test]
    fn test_tool_output_mentioning_declined_is_not_a_denial() {
        let theme = Theme::default();
        let msg = Message::tool_response("call_1", "commit abc\n    payment declined by bank\ncommit def");
        let text = plain(&render_single_message_with_pending(0, &msg, None, &theme, 80, None, false)).join("\n");
        assert!(!text.contains("Execution declined by user"));
        assert!(text.contains("Output (3 lines)"));

        let denied = Message::tool_response("call_2", "Tool execution denied by user with instruction: use rg");
        let text = plain(&render_single_message_with_pending(0, &denied, None, &theme, 80, None, false)).join("\n");
        assert!(text.contains("Execution declined by user: use rg"));
    }

    #[test]
    fn test_skill_detection_uses_the_answered_call_name() {
        let theme = Theme::default();
        let msg = Message::tool_response("call_1", "Description: Specialized procedural skill for docs\nmore");
        let shown = render_single_message_with_pending_and_thinking(
            0, &msg, None, &theme, 80, None, false, false, Some(("read_file", "{}")),
        );
        assert!(plain(&shown).join("\n").contains("Specialized procedural skill"));

        let hidden = render_single_message_with_pending_and_thinking(
            0, &msg, None, &theme, 80, None, false, false, Some(("activate_skill", "{}")),
        );
        assert!(!plain(&hidden).join("\n").contains("Specialized procedural skill"));
    }

    #[test]
    fn test_subagent_report_is_tagged_with_its_role() {
        let theme = Theme::default();
        let msg = Message::tool_response("call_1", "Subagent 'sub_9' completed:\n\nall good");
        let lines = render_single_message_with_pending_and_thinking(
            0, &msg, None, &theme, 80, None, false, false,
            Some(("delegate_subagent", r#"{"role":"SCOUT","prompt":"x"}"#)),
        );
        assert!(plain(&lines)[0].contains("[scout:sub_9]"));
    }

    #[test]
    fn test_system_message_with_inline_hash_is_not_markdown() {
        let theme = Theme::default();
        let lines = format_system_message("Could not sync issue #12 --- retry later", &theme, 80, false);
        let text = plain(&lines);
        assert_eq!(text.len(), 1);
        assert!(text[0].starts_with("  · "), "{:?}", text);

        let md = format_system_message("## Summary\nok", &theme, 80, false);
        assert!(!plain(&md)[0].starts_with("  · "));
    }

    #[tokio::test]
    async fn test_scroll_offset_survives_more_than_u16_lines() {
        let client = LlmClient::new(corex_core::config::Config::default());
        let mut app = App::new(client, PathBuf::from("/tmp"), false);
        app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
        app.total_rendered_items = 100_000;
        assert_eq!(app.max_scroll_offset(), 99_990);

        app.scroll_to_bottom();
        assert_eq!(app.scroll_offset, 99_990);
        app.scroll_chat(-30);
        assert_eq!(app.scroll_offset, 99_960);
        assert!(!app.auto_scroll);
    }

    async fn idle_app() -> App {
        let client = LlmClient::new(corex_core::config::Config::default());
        App::new(client, PathBuf::from("/tmp"), false)
    }

    #[tokio::test]
    async fn test_steer_messages_reach_the_model_as_one_user_message() {
        let mut app = idle_app().await;
        assert!(!app.deliver_steer(), "nothing to deliver");

        app.steer_messages.push("first".to_string());
        app.steer_messages.push("second".to_string());
        app.message_queue.push_back("later".to_string());
        let before = app.session.messages.len();

        assert!(app.deliver_steer());
        assert_eq!(app.session.messages.len(), before + 1);
        let last = app.session.messages.last().unwrap();
        assert_eq!(last.role, "user");
        assert_eq!(last.text_content(), Some("first\n\nsecond"));
        // The queue is untouched: it still waits for the end of the answer.
        assert_eq!(app.message_queue.len(), 1);
        assert!(app.steer_messages.is_empty());
    }

    #[tokio::test]
    async fn test_send_now_promotes_the_oldest_queued_message() {
        let mut app = idle_app().await;
        assert!(!app.promote_queued_to_steer());
        app.message_queue.push_back("one".to_string());
        app.message_queue.push_back("two".to_string());

        assert!(app.promote_queued_to_steer());
        assert_eq!(app.steer_messages, vec!["one".to_string()]);
        assert_eq!(app.message_queue.len(), 1);
        assert_eq!(app.message_queue.front().map(String::as_str), Some("two"));
    }

    #[tokio::test]
    async fn test_esc_takes_back_the_newest_pending_message_first() {
        let mut app = idle_app().await;
        app.message_queue.push_back("queued".to_string());
        app.steer_messages.push("steer".to_string());
        assert!(app.has_pending_messages());

        assert_eq!(app.undo_last_pending().as_deref(), Some("steer"));
        assert_eq!(app.undo_last_pending().as_deref(), Some("queued"));
        assert_eq!(app.undo_last_pending(), None);
        assert!(!app.has_pending_messages());
    }

    #[tokio::test]
    async fn test_restored_messages_never_overwrite_what_is_typed() {
        let mut app = idle_app().await;
        app.restore_to_input(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(app.input_buffer, "a\nb");
        assert_eq!(app.cursor_idx, app.input_buffer.len());

        app.input_buffer = "half typed".to_string();
        app.restore_to_input(vec!["taken back".to_string()]);
        assert_eq!(app.input_buffer, "taken back\nhalf typed");

        let before = app.input_buffer.clone();
        app.restore_to_input(vec![]);
        assert_eq!(app.input_buffer, before);
    }

    #[tokio::test]
    async fn test_cancelling_keeps_partial_text_and_returns_every_pending_message() {
        let mut app = idle_app().await;
        app.is_streaming = true;
        app.streaming_text = "I was in the middle of".to_string();
        app.steer_messages.push("steer".to_string());
        app.message_queue.push_back("queued".to_string());
        let generation = app.current_generation_id;

        let undelivered = app.take_all_pending();
        assert_eq!(undelivered, vec!["steer".to_string(), "queued".to_string()], "delivery order");
        app.cancel_generation("cancelled");
        app.restore_to_input(undelivered);

        assert!(!app.is_streaming);
        assert_ne!(app.current_generation_id, generation, "late events of the old turn are dropped");
        let last = app.session.messages.last().unwrap();
        assert_eq!(last.role, "assistant");
        assert!(last.text_content().unwrap().ends_with("*(interrupted)*"));
        assert_eq!(app.input_buffer, "steer\nqueued");
        assert!(app.streaming_text.is_empty());
    }

    #[tokio::test]
    async fn test_cancelling_without_partial_text_leaves_a_note() {
        let mut app = idle_app().await;
        app.is_streaming = true;
        app.cancel_generation("Generation interrupted to deliver your message.");
        let last = app.session.messages.last().unwrap();
        assert_eq!(last.role, "system");
        assert_eq!(last.text_content(), Some("Generation interrupted to deliver your message."));
    }

    fn screen_text(app: &mut App, width: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        terminal.draw(|f| render_ui(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn test_busy_agent_tells_what_enter_and_alt_enter_do() {
        let mut app = idle_app().await;

        // Idle: no mention of queueing at all.
        assert!(!screen_text(&mut app, 110).contains("Alt+Enter"));

        // Busy with an empty prompt: the placeholder explains it.
        app.is_streaming = true;
        let empty = screen_text(&mut app, 110);
        assert!(empty.contains("Type to queue a message · Alt+Enter sends it now"), "{empty}");

        // Busy while typing: the status bar spells out the keys, and nothing is appended to the
        // text itself (the same hint twice, glued to what is being typed, was only clutter).
        app.input_buffer = "check the logs too".to_string();
        app.cursor_idx = app.input_buffer.len();
        let typing = screen_text(&mut app, 110);
        assert!(!typing.contains("Enter queues"), "{typing}");
        // The status line on the left is long here (no API key): a shorter hint takes the slot.
        assert!(typing.contains("(Alt+Enter: send now"), "{typing}");

        // Once a message waits in the queue, the status bar offers to send it now.
        app.input_buffer.clear();
        app.cursor_idx = 0;
        app.message_queue.push_back("check the logs too".to_string());
        let queued = screen_text(&mut app, 170);
        assert!(queued.contains("Alt+Enter: send now · Esc: undo"), "{queued}");
        assert!(queued.contains("[queued]") && queued.contains("(after the answer)"), "{queued}");

        // And once promoted, the interrupt shortcut is the one on offer.
        app.promote_queued_to_steer();
        let steering = screen_text(&mut app, 170);
        assert!(steering.contains("[sending]") && steering.contains("Alt+Enter: interrupt now"), "{steering}");

        // On a crowded status line the hint shrinks but the shortcut stays visible.
        let crowded = screen_text(&mut app, 110);
        assert!(crowded.contains("Esc: undo"), "{crowded}");
    }

    #[test]
    fn test_completion_badges_stack_without_blank_rows() {
        let theme = Theme::default();
        let render = |first: &str, next: &str| {
            let (a, b) = (Message::system(first), Message::system(next));
            render_single_message_with_pending(0, &a, Some(&b), &theme, 80, None, false)
        };
        let done = "TASK_DONE|42|0|1s";
        // Both spellings of a subagent badge are treated alike.
        for badge in ["SUBAGENT_DONE|sub_1|SCOUT|1s\nok", "SUBAGENTDONE|sub_1|SCOUT|1s\nok"] {
            let stacked = render(badge, done);
            assert_ne!(plain(&stacked).last().map(String::as_str), Some(""), "{badge}");
        }
        let separated = render(done, "Activated model x");
        assert_eq!(plain(&separated).last().map(String::as_str), Some(""));
    }

    #[test]
    fn test_collapsed_log_keeps_last_rows_behind_a_marker() {
        let theme = Theme::default();
        let body = (1..=9).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
        let msg = format!("TASK_DONE|7|0|2s\n{body}");

        let collapsed = plain(&format_system_message(&msg, &theme, 80, false));
        assert!(collapsed.iter().any(|l| l.contains("(+4 lines above)")), "{collapsed:?}");
        assert!(collapsed.iter().any(|l| l.contains("row 9")));
        assert!(!collapsed.iter().any(|l| l.contains("row 4")));

        let expanded = plain(&format_system_message(&msg, &theme, 80, true));
        assert!(expanded.iter().any(|l| l.contains("row 1")));
        assert!(!expanded.iter().any(|l| l.contains("lines above")));
    }

    #[test]
    fn test_truncate_to_width_counts_columns() {
        assert_eq!(truncate_to_width("short", 10), "short");
        assert_eq!(truncate_to_width("ññññññññññ", 10), "ññññññññññ");
        let cut = truncate_to_width("日本語日本語日本語", 10);
        assert!(unicode_width::UnicodeWidthStr::width(cut.as_str()) <= 10);
        assert!(cut.ends_with("..."));
    }

    #[tokio::test]
    async fn test_pasted_images_travel_with_the_prompt_that_names_them() {
        let mut app = idle_app().await;
        let image = clipboard::ClipboardImage { media_type: "image/png", data: vec![1, 2, 3] };
        app.images.insert(1, image.clone());
        app.images.insert(2, image);

        // No tag, no image: the prompt stays plain text.
        assert_eq!(app.user_message("hello".to_string()).text_content(), Some("hello"));

        let msg = app.user_message("what is in [Image #1] ?".to_string());
        // The words (tag included) stay readable for the feed, titles and summaries.
        assert_eq!(msg.display_text().as_deref(), Some("what is in [Image #1] ?"));
        match msg.content {
            Some(corex_core::types::MessageContent::Parts(parts)) => {
                assert_eq!(parts.len(), 2);
                let url = &parts[1].image_url.as_ref().expect("image part").url;
                assert_eq!(url, "data:image/png;base64,AQID");
            }
            other => panic!("expected multipart content, got {:?}", other),
        }
        // Only the image the prompt names was used.
        assert!(!app.images.contains_key(&1));
        assert!(app.images.contains_key(&2));
    }

    #[tokio::test]
    async fn test_deleting_an_image_tag_forgets_its_image() {
        let mut app = idle_app().await;
        let image = clipboard::ClipboardImage { media_type: "image/png", data: vec![9] };
        app.images.insert(4, image);
        app.pastes.insert(7, "some pasted text".to_string());
        app.forget_paste_tag("[Image #4]");
        app.forget_paste_tag("[Pasted text #7 +3 lines]");
        assert!(app.images.is_empty() && app.pastes.is_empty());
    }

    #[test]
    fn test_image_tags_are_found_by_id_and_styled_like_paste_tags() {
        assert_eq!(image_tag_ids("a [Image #1] b [Image #12] [Image #x] [Image #3"), vec![1, 12]);
        assert_eq!(paste_tag_start("see [Image #2] now"), Some(4));
        assert!(starts_with_paste_tag("[Image #2] now"));

        let theme = Theme::default();
        let spans = format_input_spans("see [Image #2] now", &theme);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[1].content, "[Image #2]");
        assert_ne!(spans[1].style, spans[0].style);
    }

    #[test]
    fn test_image_tags_stand_out_in_the_conversation_feed() {
        let theme = Theme::default();
        let msg = Message::user_parts(vec![
            corex_core::types::ContentPart::text("what is this? [Image #1]"),
            corex_core::types::ContentPart::image_url("data:image/png;base64,AAAA"),
        ]);
        let lines = render_single_message_with_pending(0, &msg, None, &theme, 80, None, false);
        // Wrapping cuts the line into words, each keeping the style of the span it came from.
        let drawn = |text: &str| {
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .find(|span| span.content == text)
                .unwrap_or_else(|| panic!("{text:?} is drawn"))
        };
        // Same highlight as in the composer, not the plain text colour of the prompt.
        assert!(drawn("[Image").style.bg.is_some());
        assert_eq!(drawn("[Image").style, drawn("#1]").style);
        assert!(drawn("what").style.bg.is_none());

        // The message a tool uses to hand over its images is not the user's and stays out of the feed.
        let tool_images = {
            corex_core::push_tool_image("a.png", "data:image/png;base64,AAAA");
            corex_core::drain_tool_images_message().expect("queued image")
        };
        assert!(render_single_message_with_pending(0, &tool_images, None, &theme, 80, None, false).is_empty());
    }

#[test]
fn tap_pauses_mouse_reporting_then_resumes_it() {
    use crate::app::mobile::TapKeyboardGate;
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let (mut gate, mut capture) = (TapKeyboardGate::default(), true);

    gate.on_tap(t0, &mut capture);
    assert!(!capture, "Termux raises the keyboard only while reporting is off");
    gate.resume_if_due(t0 + Duration::from_millis(300), &mut capture);
    assert!(!capture, "the tap is not confirmed yet");
    gate.resume_if_due(t0 + Duration::from_millis(800), &mut capture);
    assert!(capture, "swipes are wheel events again");

    // Reporting the user turned off stays off: a tap must not switch it on later.
    let mut capture = false;
    gate.on_tap(t0, &mut capture);
    gate.resume_if_due(t0 + Duration::from_secs(10), &mut capture);
    assert!(!capture);

    // F2 during a pause wins over the pending resume.
    let mut capture = true;
    gate.on_tap(t0, &mut capture);
    gate.cancel();
    gate.resume_if_due(t0 + Duration::from_secs(10), &mut capture);
    assert!(!capture);
}

#[test]
fn arrows_right_after_a_tap_count_as_a_swipe_only_inside_the_window() {
    use crate::app::mobile::TapKeyboardGate;
    use std::time::{Duration, Instant};

    let t0 = Instant::now();
    let (mut gate, mut capture) = (TapKeyboardGate::default(), true);
    assert!(!gate.in_tap_window(t0));
    gate.on_tap(t0, &mut capture);
    assert!(gate.in_tap_window(t0 + Duration::from_millis(100)));
    assert!(!gate.in_tap_window(t0 + Duration::from_millis(800)));

    // `/keyboard` waits much longer, but arrow keys keep working during that wait.
    let (mut gate, mut capture) = (TapKeyboardGate::default(), true);
    gate.request_keyboard(t0, &mut capture);
    assert!(!capture);
    assert!(!gate.in_tap_window(t0 + Duration::from_millis(100)));
}

#[tokio::test]
async fn test_directory_add_validates_and_lists_roots() {
    let base = std::env::temp_dir().join(format!("corex-dir-add-{}", std::process::id()));
    let (ws, data) = (base.join("ws"), base.join("data"));
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("notes.txt"), "x").unwrap();
    let mut app = App::new(LlmClient::new(corex_core::config::Config::default()), ws.clone(), false);

    let added = app.add_directory("../data");
    let file = app.add_directory("../data/notes.txt");
    let inside = app.add_directory("src");
    let again = app.add_directory(&data.display().to_string());
    let missing = app.add_directory("../nope");
    let report = app.directory_report();
    let _ = std::fs::remove_dir_all(&base);

    assert_eq!(added, Ok(data.canonicalize().unwrap_or(data.clone())));
    assert!(file.is_err());
    assert_eq!(inside, Err("it is already readable".to_string()));
    assert_eq!(again, Err("it is already readable".to_string()));
    assert!(missing.is_err());
    assert_eq!(app.extra_dirs.len(), 1);
    assert!(report.contains("Additional directories"));
}

#[tokio::test]
async fn test_losing_focus_mid_drag_drops_the_selection() {
    let mut app = App::new(LlmClient::new(corex_core::config::Config::default()), PathBuf::from("/tmp"), false);
    app.last_chat_rect = Some(Rect::new(0, 0, 80, 10));
    app.selection_lines.tail = vec!["some text to select".to_string()];
    app.begin_selection(2, 0);
    app.extend_selection(10, 0);

    app.cancel_drag();

    assert!(app.selection.is_none());
}

#[tokio::test]
async fn test_rewind_reverts_files_and_returns_the_prompt() {
    use crate::rewind_dialog::RewindAction;
    use corex_core::file_history::FileEdit;

    let ws = std::env::temp_dir().join(format!("corex-rewind-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let (kept, created) = (ws.join("kept.rs"), ws.join("created.rs"));
    std::fs::write(&kept, "v3\n").unwrap();
    std::fs::write(&created, "new\n").unwrap();

    let mut app = App::new(LlmClient::new(corex_core::config::Config::default()), ws.clone(), false);
    for t in ["make v2", "make v3 and a new file"] {
        app.session.add_message(Message::user(t));
        app.session.add_message(Message::assistant("done", None));
    }
    app.session.file_edits = vec![
        FileEdit { turn: 1, path: kept.clone(), before: Some("v1\n".into()), after: Some("v2\n".into()) },
        FileEdit { turn: 2, path: kept.clone(), before: Some("v2\n".into()), after: Some("v3\n".into()) },
        FileEdit { turn: 2, path: created.clone(), before: None, after: Some("new\n".into()) },
    ];

    app.apply_rewind(RewindAction::RewindAndRevert, 2);
    let kept_now = std::fs::read_to_string(&kept).unwrap();
    let created_exists = created.exists();
    let _ = std::fs::remove_dir_all(&ws);

    assert_eq!(kept_now, "v2\n");
    assert!(!created_exists);
    assert_eq!(app.input_buffer, "make v3 and a new file");
    assert_eq!(app.session.user_turns().len(), 1);
    assert_eq!(app.session.file_edits.len(), 1, "turn 1 can still be rewound later");
    assert!(!app.rewind_dialog.is_open);
}
