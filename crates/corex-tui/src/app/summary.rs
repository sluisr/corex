//! Session summary reporting printed after TUI exit.

use std::io::IsTerminal;

use corex_core::pricing::{model_display_label, ModelSpend, Spend};

use crate::app::App;
use crate::text::group_thousands;

/// Label column of the summary, padded so the values line up.
const LABEL_WIDTH: usize = 8;

/// Prints the final session summary to standard output after restoring the terminal,
/// so it remains visible in the shell scrollback.
pub fn print_session_summary(app: &App) {
    let color = std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal();
    println!();
    print!("{}", format_session_summary(app, color));
    println!();
}

/// A dollar amount with the precision these costs need: fractions of a cent matter.
fn money(amount: f64) -> String {
    if amount <= 0.0 {
        "$0.00".to_string()
    } else if amount < 0.0001 {
        "<$0.0001".to_string()
    } else {
        format!("${:.4}", amount)
    }
}

/// `4m 20s`, `2h 5m`, or just `45s` under a minute.
fn duration_label(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

fn cost_label(spend: &Spend) -> String {
    let u = &spend.usage;
    if u.total_tokens == 0 {
        return "$0.00".to_string();
    }
    if spend.local_tokens == u.total_tokens {
        return "free (local model)".to_string();
    }
    if spend.unpriced_tokens == u.total_tokens {
        return "billed by the provider".to_string();
    }
    let mut text = money(spend.cost);
    if spend.saved > 0.00001 {
        text.push_str(&format!(" · {} saved by cache", money(spend.saved)));
    }
    if spend.unpriced_tokens > 0 {
        text.push_str(" · plus provider-billed tokens");
    }
    text
}

fn tokens_label(u: &corex_core::types::Usage) -> String {
    if u.total_tokens == 0 {
        "none".to_string()
    } else {
        let mut text = format!(
            "{} in · {} out",
            group_thousands(u.prompt_tokens),
            group_thousands(u.completion_tokens)
        );
        if u.prompt_tokens > 0 && u.prompt_cache_hit_tokens > 0 {
            text.push_str(&format!(" · {:.1}% cached", u.cache_hit_percentage()));
        }
        text
    }
}

/// The summary as the lines to print: dim labels, plain values, nothing decorative.
pub fn format_session_summary(app: &App, color: bool) -> String {
    format_summary_with_models(
        app,
        &app.llm_client.spend(),
        &app.llm_client.model_spends(),
        app.launched_at.elapsed().as_secs(),
        color,
    )
}

#[cfg(test)]
fn format_summary(app: &App, spend: &Spend, elapsed: u64, color: bool) -> String {
    format_summary_with_models(app, spend, &app.llm_client.model_spends(), elapsed, color)
}

fn format_summary_with_models(
    app: &App,
    spend: &Spend,
    model_spends: &[ModelSpend],
    elapsed: u64,
    color: bool,
) -> String {
    let cfg = app.llm_client.get_config();
    let active_display = model_display_label(&cfg);

    let model_display = match model_spends {
        [only] if only.label != active_display => format!("{} → {}", only.label, active_display),
        [only] => only.label.clone(),
        _ => active_display,
    };

    let prompts = app.prompts_this_run;
    let tokens = tokens_label(&spend.usage);

    let mut rows: Vec<(&str, String)> = Vec::new();
    if model_spends.len() > 1 {
        for (idx, ms) in model_spends.iter().enumerate() {
            let label = if idx == 0 { "Models" } else { "" };
            rows.push((
                label,
                format!(
                    "{} ({} · {})",
                    ms.label,
                    tokens_label(&ms.spend.usage),
                    cost_label(&ms.spend)
                ),
            ));
        }
    } else {
        rows.push(("Model", model_display));
    }
    rows.push((
        "Session",
        format!(
            "{} · {} {}",
            duration_label(elapsed),
            prompts,
            if prompts == 1 { "prompt" } else { "prompts" }
        ),
    ));
    rows.push(("Tokens", tokens));
    rows.push(("Cost", cost_label(spend)));
    // A session without user turns is not worth offering as a resume target.
    let resumable = app.session.messages.iter().any(|m| m.role == "user");
    if resumable {
        rows.push(("Resume", format!("cx --resume {}", app.session.id)));
    }

    let mut out = String::new();
    for (idx, (label, value)) in rows.iter().enumerate() {
        // A blank line sets the way back into the session apart from the numbers above it.
        if resumable && idx == rows.len() - 1 {
            out.push('\n');
        }
        let label = format!("{:<width$}", label, width = LABEL_WIDTH);
        if color {
            out.push_str(&format!("  \x1b[90m{}\x1b[0m {}\n", label, value));
        } else {
            out.push_str(&format!("  {} {}\n", label, value));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use corex_core::client::LlmClient;
    use corex_core::pricing::{deepseek_rates_at, Rates};
    use corex_core::types::Usage;
    use std::path::PathBuf;

    async fn app() -> App {
        App::new(LlmClient::new(corex_core::config::Config::default()), PathBuf::from("/tmp"), false)
    }

    fn off_peak() -> chrono::DateTime<chrono::Utc> {
        "2026-11-07T12:00:00Z".parse().unwrap()
    }

    fn usage(prompt: u64, hit: u64, out: u64) -> Usage {
        Usage {
            prompt_tokens: prompt,
            prompt_cache_hit_tokens: hit,
            prompt_cache_miss_tokens: prompt - hit,
            completion_tokens: out,
            total_tokens: prompt + out,
        }
    }

    #[tokio::test]
    async fn the_summary_is_plain_aligned_rows() {
        let mut app = app().await;
        app.prompts_this_run = 3;
        app.session.add_message(corex_core::types::Message::user("hola".to_string()));
        let mut spend = Spend::default();
        spend.add(&usage(30_623, 28_416, 2_778), Some(deepseek_rates_at("deepseek-flash", off_peak())));

        let text = format_summary(&app, &spend, 72, false);
        let rows: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(rows.len(), 5, "{text}");
        assert!(rows[0].starts_with("  Model    "), "{text}");
        assert!(text.contains("1m 12s · 3 prompts"), "{text}");
        assert!(text.contains("30,623 in · 2,778 out · 92.8% cached"), "{text}");
        assert!(text.contains("saved by cache"), "{text}");
        assert!(text.contains(&format!("cx --resume {}", app.session.id)), "{text}");
        assert!(!text.contains('✦') && !text.contains('\x1b'), "{text}");
        let columns: Vec<usize> = rows.iter().map(|r| r.len() - r.trim_start().len() + LABEL_WIDTH + 1).collect();
        assert!(columns.iter().all(|c| *c == columns[0]));
    }

    #[tokio::test]
    async fn a_session_without_turns_reports_nothing_spent() {
        let app = app().await;
        let text = format_session_summary(&app, false);
        assert!(text.contains("Tokens   none"), "{text}");
        assert!(text.contains("Cost     $0.00"), "{text}");
        assert!(text.contains("0s · 0 prompts"), "{text}");
        assert!(!text.contains("--resume"), "{text}");
    }

    #[tokio::test]
    async fn a_resumed_session_reports_only_this_run() {
        let mut app = app().await;
        app.session.created_at = chrono::Utc::now() - chrono::Duration::days(2);
        app.session.update_usage(&usage(900_000, 0, 9_000));
        let text = format_session_summary(&app, false);
        assert!(text.contains("Session  0s"), "{text}");
        assert!(text.contains("Tokens   none"), "{text}");
    }

    #[test]
    fn costs_of_mixed_sources_are_labelled() {
        let mut spend = Spend::default();
        spend.add(&usage(100, 0, 10), Some(Rates::FREE));
        assert_eq!(cost_label(&spend), "free (local model)");
        let mut spend = Spend::default();
        spend.add(&usage(100, 0, 10), None);
        assert_eq!(cost_label(&spend), "billed by the provider");
        spend.add(&usage(1_000_000, 0, 0), Some(deepseek_rates_at("deepseek-flash", off_peak())));
        assert_eq!(cost_label(&spend), "$0.1500 · plus provider-billed tokens");
    }

    #[test]
    fn money_keeps_fractions_of_a_cent() {
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(0.00001), "<$0.0001");
        assert_eq!(money(0.001485), "$0.0015");
        assert_eq!(money(1.5), "$1.5000");
    }

    #[test]
    fn durations_grow_into_hours() {
        assert_eq!(duration_label(45), "45s");
        assert_eq!(duration_label(260), "4m 20s");
        assert_eq!(duration_label(2 * 86_400 + 300), "48h 5m");
    }

    #[tokio::test]
    async fn multi_provider_session_lists_each_model_used() {
        let mut app = app().await;
        app.prompts_this_run = 2;
        app.session.add_message(corex_core::types::Message::user("hola".to_string()));
        let mut total = Spend::default();
        let mut ds = Spend::default();
        ds.add(&usage(10_000, 8_000, 500), Some(deepseek_rates_at("deepseek-flash", off_peak())));
        total.add(&usage(10_000, 8_000, 500), Some(deepseek_rates_at("deepseek-flash", off_peak())));
        let mut az = Spend::default();
        az.add(&usage(5_000, 2_500, 400), None);
        total.add(&usage(5_000, 2_500, 400), None);

        let models = vec![
            ModelSpend { label: "DeepSeek-V4.1-Flash".to_string(), requests: 1, spend: ds },
            ModelSpend { label: "Azure AI Foundry · DeepSeek V4.1 Flash".to_string(), requests: 1, spend: az },
        ];
        let text = format_summary_with_models(&app, &total, &models, 95, false);
        assert!(text.contains("Models   DeepSeek-V4.1-Flash (10,000 in · 500 out · 80.0% cached"), "{text}");
        assert!(text.contains("Azure AI Foundry · DeepSeek V4.1 Flash (5,000 in · 400 out · 50.0% cached · billed by the provider)"), "{text}");
        assert!(text.contains("15,000 in · 900 out"), "{text}");
    }
}
