use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render_gradient_logo(version: &str, authenticated: bool, local_mode: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Top padding line
    lines.push(Line::from(""));

    let c1 = Color::Rgb(30, 58, 138);   // #1e3a8a
    let c2 = Color::Rgb(37, 99, 235);   // #2563eb
    let c3 = Color::Rgb(147, 197, 253); // #93c5fd

    // Row 1: "\u{200B}▝▜▄     UTI CLI v<version>"
    lines.push(Line::from(vec![
        Span::styled(" \u{259D}\u{259C}\u{2584}", Style::default().fg(c1).add_modifier(Modifier::BOLD)),
        Span::raw("     "),
        Span::styled("UTI CLI", Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" v{}", version), Style::default().fg(Color::DarkGray)),
    ]));

    // Row 2: "   ▝▜▄"
    lines.push(Line::from(vec![
        Span::styled("   \u{259D}\u{259C}\u{2584}", Style::default().fg(c2).add_modifier(Modifier::BOLD)),
    ]));

    // Row 3: Dynamic authentication status
    let (status_text, cmd_hint, status_color) = if local_mode {
        ("Local LLM Mode (Offline)", " /model", Color::Rgb(105, 240, 174))
    } else if authenticated {
        ("Authenticated with DeepSeek API Key", " /key", Color::Reset)
    } else {
        ("API Key Missing (Type /key <sk-...> to connect)", " /key", Color::Rgb(255, 170, 0))
    };

    lines.push(Line::from(vec![
        Span::styled("  \u{2597}\u{259F}\u{2580}", Style::default().fg(c3).add_modifier(Modifier::BOLD)),
        Span::raw("    "),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::styled(cmd_hint, Style::default().fg(Color::DarkGray)),
    ]));

    // Row 4: " ▝▀"
    lines.push(Line::from(vec![
        Span::styled(" \u{259D}\u{2580}", Style::default().fg(c3).add_modifier(Modifier::BOLD)),
    ]));

    // Bottom padding line for clean breathing room above chat feed
    lines.push(Line::from(""));

    lines
}
