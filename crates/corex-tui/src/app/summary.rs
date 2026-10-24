//! Session summary reporting printed after TUI exit.

use std::io::IsTerminal;

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

/// `4m 20s`, or just `45s` under a minute.
fn duration_label(seconds: i64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

/// The summary as the lines to print: dim labels, plain values, nothing decorative.
pub fn format_session_summary(app: &App, color: bool) -> String {
    let cfg = app.llm_client.get_config();
    let is_local = cfg.local_llm_enabled;
    let is_deepseek = cfg.is_deepseek_endpoint();

    let model_display = if is_local {
        if !cfg.local_llm_model.is_empty() {
            format!("Local ({})", cfg.local_llm_model)
        } else {
            "Local".to_string()
        }
    } else if is_deepseek && (cfg.model == "deepseek-flash" || cfg.model == "deepseek-v4.1-flash" || cfg.model == "deepseek-v4.1-flash-1017" || cfg.model == "deepseek-v4-flash" || cfg.model == "deepseek-chat") {
        if cfg.model == "deepseek-v4.1-flash-1017" {
            "DeepSeek-V4.1-Flash-1017".to_string()
        } else {
            "DeepSeek-V4.1-Flash".to_string()
        }
    } else if is_deepseek && (cfg.model == "deepseek-v4-pro" || cfg.model == "deepseek-reasoner") {
        "DeepSeek-V4-Pro".to_string()
    } else {
        let all = cfg.all_providers();
        let prov = cfg.active_provider.as_deref().and_then(|p_name| all.iter().find(|p| p.name == p_name));
        prov.and_then(|p| p.models.iter().find(|m| m.id == cfg.model))
            .map(|m| format!("{} · {}", prov.map(|p| p.display_name()).unwrap_or(""), m.display_name()))
            .unwrap_or_else(|| cfg.model.clone())
    };

    let elapsed = chrono::Utc::now().signed_duration_since(app.session.created_at).num_seconds().max(0);
    let messages = app.session.messages.len();
    let u = &app.session.total_usage;

    let tokens = if u.total_tokens == 0 {
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
    };

    let cost = if is_local {
        "free (local model)".to_string()
    } else if u.total_tokens > 0 && is_deepseek {
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
        let uncached = (u.prompt_tokens as f64 * miss_rate) + (u.completion_tokens as f64 * comp_rate);
        let saved = (uncached - actual_cost).max(0.0);
        if saved > 0.00001 {
            format!("{} · {} saved by cache", money(actual_cost), money(saved))
        } else {
            money(actual_cost)
        }
    } else if u.total_tokens > 0 {
        "billed by the provider".to_string()
    } else {
        "$0.00".to_string()
    };

    let rows = [
        ("Model", model_display),
        ("Session", format!("{} · {} messages", duration_label(elapsed), messages)),
        ("Tokens", tokens),
        ("Cost", cost),
        ("Resume", format!("cx --resume {}", app.session.id)),
    ];

    let mut out = String::new();
    for (idx, (label, value)) in rows.iter().enumerate() {
        // A blank line sets the way back into the session apart from the numbers above it.
        if idx == rows.len() - 1 {
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
    use std::path::PathBuf;

    async fn app() -> App {
        App::new(LlmClient::new(corex_core::config::Config::default()), PathBuf::from("/tmp"), false)
    }

    #[tokio::test]
    async fn the_summary_is_plain_aligned_rows() {
        let mut app = app().await;
        app.session.total_usage.prompt_tokens = 30_623;
        app.session.total_usage.completion_tokens = 2_778;
        app.session.total_usage.total_tokens = 33_401;
        app.session.total_usage.prompt_cache_hit_tokens = 28_416;

        let text = format_session_summary(&app, false);
        let rows: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(rows.len(), 5, "{text}");
        assert!(rows[0].starts_with("  Model    "), "{text}");
        assert!(text.contains("30,623 in · 2,778 out · 92.8% cached"), "{text}");
        assert!(text.contains("saved by cache"), "{text}");
        assert!(text.contains(&format!("cx --resume {}", app.session.id)), "{text}");
        // Nothing decorative and no colour when it is not asked for.
        assert!(!text.contains('✦') && !text.contains('\x1b'), "{text}");
        // Every value starts in the same column.
        let columns: Vec<usize> = rows.iter().map(|r| r.len() - r.trim_start().len() + LABEL_WIDTH + 1).collect();
        assert!(columns.iter().all(|c| *c == columns[0]));
    }

    #[tokio::test]
    async fn a_session_without_turns_reports_nothing_spent() {
        let app = app().await;
        let text = format_session_summary(&app, false);
        assert!(text.contains("Tokens   none"), "{text}");
        assert!(text.contains("Cost     $0.00"), "{text}");
    }

    #[test]
    fn money_keeps_fractions_of_a_cent() {
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(0.00001), "<$0.0001");
        assert_eq!(money(0.001485), "$0.0015");
        assert_eq!(money(1.5), "$1.5000");
    }

    #[test]
    fn short_sessions_are_counted_in_seconds() {
        assert_eq!(duration_label(45), "45s");
        assert_eq!(duration_label(260), "4m 20s");
    }
}
