use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Renders the individual letters of "Corex", each with its own soft shade of
/// satin gray, animated with a gentle, slow luminescence wave across the letters.
pub fn render_corex_title_spans(elapsed_secs: f32) -> Vec<Span<'static>> {
    const CHARS: [&str; 5] = ["C", "o", "r", "e", "x"];

    // Ultra-soft, understated satin-slate tones (subtle ~10-unit gradient across letters)
    let base_grays: [(u8, u8, u8); 5] = [
        (192, 198, 208),  // C: soft light satin
        (182, 188, 198),  // o: slate tint
        (172, 178, 188),  // r: medium slate
        (162, 168, 178),  // e: subdued steel
        (152, 158, 168),  // x: muted graphite
    ];

    // Soft moonlight shimmer peak (gentle +36 brightness, soothing liquid glow)
    let highlight = (228, 234, 244);

    // Continuous, smoothly flowing 3.2-second fluid wave with seamless looping
    let cycle_time = 3.2;
    let progress = (elapsed_secs % cycle_time) / cycle_time;
    let wave_pos = progress * 5.6 - 0.8;

    let mut spans = Vec::with_capacity(CHARS.len());

    for (i, &ch) in CHARS.iter().enumerate() {
        let dist = (wave_pos - i as f32).abs();
        let factor = if dist < 1.8 {
            0.5 * (1.0 + (std::f32::consts::PI * dist / 1.8).cos())
        } else {
            0.0
        };

        let (base_r, base_g, base_b) = base_grays[i];
        let (hi_r, hi_g, hi_b) = highlight;

        let r = (base_r as f32 + (hi_r as f32 - base_r as f32) * factor).round() as u8;
        let g = (base_g as f32 + (hi_g as f32 - base_g as f32) * factor).round() as u8;
        let b = (base_b as f32 + (hi_b as f32 - base_b as f32) * factor).round() as u8;

        // Constant bold modifier prevents terminal font kerning jitter
        let style = Style::default().fg(Color::Rgb(r, g, b)).add_modifier(Modifier::BOLD);

        spans.push(Span::styled(ch, style));
    }

    spans
}

pub fn render_gradient_logo(
    version: &str,
    model: &str,
    authenticated: bool,
    local_mode: bool,
    update_notice: Option<&str>,
    elapsed_secs: f32,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Top padding line
    lines.push(Line::from(""));

    // Subtle, cohesive palette with muted contrast
    let cyan = Color::Rgb(130, 185, 215);        // Soft tranquil arctic mist
    let muted_gray = Color::Rgb(120, 130, 142);  // Understated neutral gray
    let sep_gray = Color::Rgb(85, 93, 105);      // Discreet separator dots

    let engine_mode = if local_mode { " (Local)" } else { "" };

    let mut line_spans = Vec::new();

    // Clean margin indentation - no bulky or distracting bullets
    line_spans.push(Span::raw("  "));

    // Animated individual letters for "Corex" with gentle, soothing transitions
    line_spans.extend(render_corex_title_spans(elapsed_secs));

    line_spans.push(Span::styled(format!(" v{}", version), Style::default().fg(muted_gray)));
    line_spans.push(Span::styled(" · ", Style::default().fg(sep_gray)));
    line_spans.push(Span::styled(model.to_string(), Style::default().fg(cyan)));
    if local_mode {
        line_spans.push(Span::styled(engine_mode, Style::default().fg(Color::Rgb(110, 190, 160))));
    }
    line_spans.push(Span::styled(" · ", Style::default().fg(sep_gray)));
    line_spans.push(Span::styled("/help", Style::default().fg(muted_gray)));

    if !authenticated && !local_mode {
        line_spans.push(Span::styled(" · ", Style::default().fg(sep_gray)));
        line_spans.push(Span::styled("/key to connect", Style::default().fg(muted_gray)));
    }

    lines.push(Line::from(line_spans));

    // Optional Line 2: Update notice
    if let Some(newer) = update_notice {
        lines.push(Line::from(vec![
            Span::styled("  Update available: ", Style::default().fg(Color::Rgb(220, 180, 80)).add_modifier(Modifier::BOLD)),
            Span::styled(format!("v{} → v{} ", version, newer), Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
            Span::styled("(run 'cx update' or update your terminal)", Style::default().fg(muted_gray)),
        ]));
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
        assert_eq!(spans.len(), 5);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Corex");

        // Verify that colors are distinct grayscale/silver tones
        let spans_t0 = render_corex_title_spans(0.0);
        let spans_t1 = render_corex_title_spans(0.8);
        assert_ne!(spans_t0[0].style.fg, spans_t1[0].style.fg);
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
    }
}
