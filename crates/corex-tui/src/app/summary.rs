//! Session summary reporting printed after TUI exit.

use crate::app::App;

/// Prints the final session summary to standard output after restoring the terminal,
/// so it remains visible in the shell scrollback.
pub fn print_session_summary(app: &App) {
    let cfg = app.llm_client.get_config();
    let is_local = cfg.local_llm_enabled;

    let is_deepseek = cfg.is_deepseek_endpoint();
    let model_display = if is_local {
        if !cfg.local_llm_model.is_empty() {
            format!("Local Offline Assistant ({})", cfg.local_llm_model)
        } else {
            "Local Offline Assistant (Air-gapped SLM)".to_string()
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
    let duration_str = if elapsed < 60 {
        format!("{}s", elapsed)
    } else {
        format!("{}m {}s", elapsed / 60, elapsed % 60)
    };

    let msg_count = app.session.messages.len();
    let u = &app.session.total_usage;

    let (cost_str, tokens_str, cache_str) = if is_local {
        (
            "\x1b[38;2;105;240;174m$0.000000 USD\x1b[0m \x1b[90m(local)\x1b[0m".to_string(),
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            None,
        )
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
        let un_cached = (u.prompt_tokens as f64 * miss_rate) + (u.completion_tokens as f64 * comp_rate);
        let saved = (un_cached - actual_cost).max(0.0);

        let hit_rate = u.cache_hit_percentage();
        let cost_text = if saved > 0.00001 {
            format!("\x1b[38;2;105;240;174m${:.6} USD\x1b[0m \x1b[90m(Saved ${:.6} with KV Cache)\x1b[0m", actual_cost, saved)
        } else {
            format!("\x1b[38;2;105;240;174m${:.6} USD\x1b[0m", actual_cost)
        };

        let cache_info = if u.prompt_tokens > 0 {
            Some(format!("{:.1}% Hit Rate ({} cached / {} in)", hit_rate, u.prompt_cache_hit_tokens, u.prompt_tokens))
        } else {
            None
        };

        (
            cost_text,
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            cache_info,
        )
    } else if u.total_tokens > 0 {
        let hit_rate = u.cache_hit_percentage();
        let cache_info = if u.prompt_tokens > 0 && u.prompt_cache_hit_tokens > 0 {
            Some(format!("{:.1}% Hit Rate ({} cached / {} in)", hit_rate, u.prompt_cache_hit_tokens, u.prompt_tokens))
        } else {
            None
        };
        (
            "\x1b[90mBilled by active provider\x1b[0m".to_string(),
            format!("{} in · {} out ({} total)", u.prompt_tokens, u.completion_tokens, u.total_tokens),
            cache_info,
        )
    } else {
        (
            "\x1b[90m$0.000000 USD (No LLM turns)\x1b[0m".to_string(),
            "0 total".to_string(),
            None,
        )
    };

    println!();
    println!("\x1b[1;38;2;56;189;248m✦ Corex Session Summary\x1b[0m");
    println!("  \x1b[90mModel:\x1b[0m     \x1b[1m{}\x1b[0m", model_display);
    println!("  \x1b[90mDuration:\x1b[0m  {} \x1b[90m·\x1b[0m {} messages", duration_str, msg_count);
    println!("  \x1b[90mTokens:\x1b[0m    {}", tokens_str);
    if let Some(c_info) = cache_str {
        println!("  \x1b[90mKV Cache:\x1b[0m  {}", c_info);
    }
    println!("  \x1b[90mCost:\x1b[0m      {}", cost_str);
    println!();
    println!("  \x1b[90mResume:\x1b[0m    \x1b[38;2;135;215;215mcx --resume {}\x1b[0m", app.session.id);
    println!();
}
