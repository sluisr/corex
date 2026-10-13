use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Renders "Corex" as clean, solid satin-white spans without distracting letter shimmer.
pub fn render_corex_title_spans(_elapsed_secs: f32) -> Vec<Span<'static>> {
    vec![Span::styled(
        "Corex",
        Style::default()
            .fg(Color::Rgb(225, 230, 240))
            .add_modifier(Modifier::BOLD),
    )]
}

/// Returns the solid, static satin-gray style for the geometric "C" emblem (◢◤ / ◥◣).
pub fn render_chevron_glyph_styles(_elapsed_secs: f32) -> (Style, Style, Style, Style) {
    // 100% solid, static satin titanium gray across all facets (clean, zero animation)
    let solid_style = Style::default()
        .fg(Color::Rgb(155, 165, 178))
        .add_modifier(Modifier::BOLD);

    (solid_style, solid_style, solid_style, solid_style)
}

pub fn shorten_model_for_mobile(model: &str) -> String {
    if model.contains("claude-3-7-sonnet") {
        "Claude 3.7".to_string()
    } else if model.contains("claude-3-5-sonnet") {
        "Claude 3.5".to_string()
    } else if model.contains("claude-3-5-haiku") {
        "Claude Haiku".to_string()
    } else if model.contains("gemini-3.8-flash") {
        "Gemini 3.8F".to_string()
    } else if model.contains("gemini-3.5-flash-lite") {
        "Gemini 3.5FL".to_string()
    } else if model.contains("gemini-3.5-flash") {
        "Gemini 3.5F".to_string()
    } else if model.contains("gemini-flash-latest") {
        "Gemini Flash".to_string()
    } else if model.contains("gemini-2.5-flash") {
        "Gemini 2.5F".to_string()
    } else if model.contains("gemini-2.0-flash") {
        "Gemini 2.0F".to_string()
    } else if model.contains("gemini-1.5-pro") {
        "Gemini 1.5P".to_string()
    } else if model.contains("deepseek-flash") || model.contains("deepseek-v4.1-flash") {
        "DeepSeek Flash".to_string()
    } else if model.contains("deepseek-pro") || model.contains("deepseek-v4-pro") {
        "DeepSeek Pro".to_string()
    } else if model.contains("llama-3.3-70b") {
        "Llama 3.3".to_string()
    } else if model.contains("codestral") {
        "Codestral".to_string()
    } else if model.len() > 18 {
        format!("{}…", &model[..16])
    } else {
        model.to_string()
    }
}

pub fn render_gradient_logo(
    version: &str,
    model: &str,
    authenticated: bool,
    local_mode: bool,
    update_notice: Option<&str>,
    elapsed_secs: f32,
) -> Vec<Line<'static>> {
    render_gradient_logo_adaptive(version, model, authenticated, local_mode, update_notice, elapsed_secs, 120)
}

pub fn render_gradient_logo_adaptive(
    version: &str,
    model: &str,
    authenticated: bool,
    local_mode: bool,
    update_notice: Option<&str>,
    elapsed_secs: f32,
    max_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Top padding line
    lines.push(Line::from(""));

    // Subtle, cohesive palette with muted contrast
    let cyan = Color::Rgb(130, 185, 215);           // Soft tranquil arctic mist
    let muted_gray = Color::Rgb(120, 130, 142);     // Understated neutral gray
    let sep_gray = Color::Rgb(85, 93, 105);         // Discreet separator dots

    let engine_mode = if local_mode { " (Local)" } else { "" };
    let is_compact = max_width < 65;

    // Retrieve solid satin-gray style for chevron emblem
    let (c_top1, _, c_bot1, _) = render_chevron_glyph_styles(elapsed_secs);

    // Line 1: Chevron top (solid gray) + Solid Corex + version
    let mut line1 = Vec::new();
    line1.push(Span::raw("  "));
    line1.push(Span::styled("◢◤", c_top1));
    line1.push(Span::raw("  "));
    line1.push(Span::styled(
        "Corex",
        Style::default()
            .fg(Color::Rgb(225, 230, 240))
            .add_modifier(Modifier::BOLD),
    ));
    line1.push(Span::styled(format!(" v{}", version), Style::default().fg(muted_gray)));
    lines.push(Line::from(line1));

    // Line 2: Chevron bottom (solid gray) + Model + Quick Commands
    let mut line2 = Vec::new();
    line2.push(Span::raw("  "));
    line2.push(Span::styled("◥◣", c_bot1));
    line2.push(Span::raw("  "));

    if is_compact {
        let short_m = shorten_model_for_mobile(model);
        line2.push(Span::styled(short_m, Style::default().fg(cyan)));
        if local_mode {
            line2.push(Span::styled(engine_mode, Style::default().fg(Color::Rgb(110, 190, 160))));
        }
        line2.push(Span::styled(" · ", Style::default().fg(sep_gray)));
        line2.push(Span::styled("/help", Style::default().fg(muted_gray)));
        line2.push(Span::styled(" · ", Style::default().fg(sep_gray)));
        line2.push(Span::styled("/model", Style::default().fg(muted_gray)));
        if !authenticated && !local_mode {
            line2.push(Span::styled(" · ", Style::default().fg(sep_gray)));
            line2.push(Span::styled("/key", Style::default().fg(muted_gray)));
        }
    } else {
        line2.push(Span::styled(model.to_string(), Style::default().fg(cyan)));
        if local_mode {
            line2.push(Span::styled(engine_mode, Style::default().fg(Color::Rgb(110, 190, 160))));
        }
        line2.push(Span::styled(" · ", Style::default().fg(sep_gray)));
        line2.push(Span::styled("/help", Style::default().fg(muted_gray)));
        line2.push(Span::styled(" · ", Style::default().fg(sep_gray)));
        line2.push(Span::styled("/model", Style::default().fg(muted_gray)));
        if !authenticated && !local_mode {
            line2.push(Span::styled(" · ", Style::default().fg(sep_gray)));
            line2.push(Span::styled("/key to connect", Style::default().fg(muted_gray)));
        }
    }
    lines.push(Line::from(line2));

    // Optional: Update notice
    if let Some(newer) = update_notice {
        if is_compact {
            lines.push(Line::from(vec![
                Span::styled("  Update: ", Style::default().fg(Color::Rgb(220, 180, 80)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("v{} → v{} ", version, newer), Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("  Update available: ", Style::default().fg(Color::Rgb(220, 180, 80)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("v{} → v{} ", version, newer), Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
                Span::styled("(run 'cx update' or update your terminal)", Style::default().fg(muted_gray)),
            ]));
        }
    }

    // Bottom padding line for clean breathing room above chat feed
    lines.push(Line::from(""));

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_corex_title_spans() {
        let spans = render_corex_title_spans(0.0);
        assert_eq!(spans.len(), 1);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Corex");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(225, 230, 240)));
    }

    #[test]
    fn test_render_chevron_glyph_styles() {
        let (t1, t2, b1, b2) = render_chevron_glyph_styles(0.0);
        // All facets have identical solid satin titanium gray color
        assert_eq!(t1.fg, b1.fg);
        assert_eq!(t1.fg, t2.fg);
        assert_eq!(b1.fg, b2.fg);
        assert_eq!(t1.fg, Some(Color::Rgb(155, 165, 178)));

        // Fully static: stays identical over time (zero animation)
        let (w1, _, _, _) = render_chevron_glyph_styles(0.0);
        let (w2, _, _, _) = render_chevron_glyph_styles(1.8);
        assert_eq!(w1.fg, w2.fg);
    }

    #[test]
    fn test_render_gradient_logo_no_bullet() {
        let lines = render_gradient_logo("0.2.0", "deepseek-flash", false, false, None, 0.0);
        let full_text: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        
        // Assert no bullet glyphs like ● or ✦ at startup
        assert!(!full_text.contains('●'));
        assert!(!full_text.contains('✦'));
        assert!(full_text.contains("Corex"));
        assert!(full_text.contains("v0.2.0"));
        assert!(full_text.contains("deepseek-flash"));
        assert!(full_text.contains("/help"));
        assert!(full_text.contains("/model"));
        assert!(full_text.contains("◢◤"));
        assert!(full_text.contains("◥◣"));
    }
}
