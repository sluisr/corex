use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render_gradient_logo(
    version: &str,
    authenticated: bool,
    local_mode: bool,
    update_notice: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Top padding line
    lines.push(Line::from(""));

    let c1 = Color::Rgb(56, 189, 248);  // Electric Cyan (#38bdf8)
    let c2 = Color::Rgb(79, 140, 255);  // Cobalt Blue (#4f8cff)
    let c3 = Color::Rgb(129, 140, 248); // Indigo (#818cf8)
    let c4 = Color::Rgb(168, 85, 247);  // Violet (#a855f7)

    // Row 1: "  ▄██▀    COREX CLI v<version>"
    lines.push(Line::from(vec![
        Span::styled("  \u{2584}\u{2588}\u{2588}\u{2580}", Style::default().fg(c1).add_modifier(Modifier::BOLD)),
        Span::raw("    "),
        Span::styled("COREX CLI", Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" v{}", version), Style::default().fg(Color::DarkGray)),
    ]));

    // Row 2: "   ▄██▀"
    lines.push(Line::from(vec![
        Span::styled("   \u{2584}\u{2588}\u{2588}\u{2580}", Style::default().fg(c2).add_modifier(Modifier::BOLD)),
    ]));

    // Row 3: "   ▀██▄   <status_text> <cmd_hint>"
    let (status_text, cmd_hint, status_color) = if local_mode {
        ("Local LLM Mode (Offline)", " /model", Color::Rgb(105, 240, 174))
    } else if authenticated {
        ("Authenticated with DeepSeek API Key", " /key", Color::Reset)
    } else {
        ("API Key Missing (Type /key <sk-...> to connect)", " /key", Color::Rgb(255, 170, 0))
    };

    lines.push(Line::from(vec![
        Span::styled("   \u{2580}\u{2588}\u{2588}\u{2584}", Style::default().fg(c3).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::styled(cmd_hint, Style::default().fg(Color::DarkGray)),
    ]));

    // Row 4: "  ▀██▄" + optional update banner
    let mut row4_spans = vec![
        Span::styled("  \u{2580}\u{2588}\u{2588}\u{2584}", Style::default().fg(c4).add_modifier(Modifier::BOLD)),
    ];
    if let Some(newer) = update_notice {
        row4_spans.push(Span::raw("   "));
        row4_spans.push(Span::styled("⚡ Update available: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
        row4_spans.push(Span::styled(format!("v{} → v{} ", version, newer), Style::default().fg(Color::Rgb(56, 189, 248)).add_modifier(Modifier::BOLD)));
        row4_spans.push(Span::styled("(run 'cx update' or update your terminal)", Style::default().fg(Color::DarkGray)));
    }
    lines.push(Line::from(row4_spans));

    // Bottom padding line for clean breathing room above chat feed
    lines.push(Line::from(""));

    lines
}
