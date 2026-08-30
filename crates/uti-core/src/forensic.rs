use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use chrono::Local;
use directories::BaseDirs;

static LOGGER: Mutex<Option<ForensicLogger>> = Mutex::new(None);

pub struct ForensicLogger {
    log_file_path: PathBuf,
}

impl ForensicLogger {
    /// Initializes or retrieves the global forensic logger.
    /// Creates log files under `~/.uti/logs/uti-forensic-YYYY-MM-DD.log`
    /// and project-level `.uti/forensic.log` if workspace directory is supplied.
    pub fn init(workspace_dir: Option<&Path>) -> PathBuf {
        let date_str = Local::now().format("%Y-%m-%d").to_string();
        let log_dir = if let Some(dirs) = BaseDirs::new() {
            dirs.home_dir().join(".uti").join("logs")
        } else {
            PathBuf::from(".uti").join("logs")
        };

        let _ = fs::create_dir_all(&log_dir);
        let log_file_path = log_dir.join(format!("uti-forensic-{}.log", date_str));

        let mut global = LOGGER.lock().unwrap();
        *global = Some(ForensicLogger {
            log_file_path: log_file_path.clone(),
        });

        // Write session start header
        Self::raw_append(&log_file_path, &format!(
            "\n╔═══════════════════════════════════════════════════════════════════════════════════════════════════╗\n\
             ║  UTI-CLI FORENSIC AUDIT SESSION STARTED: {}                                    ║\n\
             ║  Workspace: {:<86} ║\n\
             ╚═══════════════════════════════════════════════════════════════════════════════════════════════════╝\n",
            Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            workspace_dir.map(|p| p.display().to_string()).unwrap_or_else(|| "N/A".to_string())
        ));

        log_file_path
    }

    fn raw_append(path: &Path, content: &str) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(content.as_bytes());
            let _ = file.flush();
        }
    }

    pub fn log_event(category: &str, title: &str, details: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let formatted = format!(
            "\n[{}][{:<12}] ─── {} ───\n{}\n{}\n",
            timestamp,
            category.to_uppercase(),
            title,
            details.trim(),
            "─".repeat(95)
        );

        if let Ok(guard) = LOGGER.lock() {
            if let Some(ref logger) = *guard {
                Self::raw_append(&logger.log_file_path, &formatted);
            }
        }
    }

    /// Logs an outbound LLM Request (Cloud API or Local LLM)
    pub fn log_llm_request(engine: &str, model: &str, endpoint: &str, messages_count: usize, tools_count: usize, body: &str) {
        let details = format!(
            "Engine:     {}\nModel:      {}\nEndpoint:   {}\nMessages:   {}\nTools:      {}\nPayload (Raw JSON / Formatted):\n{}",
            engine, model, endpoint, messages_count, tools_count, body
        );
        Self::log_event("LLM_REQ", &format!("OUTBOUND -> {} ({})", engine, model), &details);
    }

    /// Logs an incoming LLM Response with full forensic telemetry:
    /// - TTFT (Time To First Token in ms)
    /// - Total round-trip duration & Generation speed (Tokens/sec)
    /// - Exact token usage breakdown (Prompt, Cache Hit %, Completion, Reasoning)
    /// - Real-time estimated financial cost ($USD) & cache savings
    pub fn log_llm_response(
        engine: &str,
        model: &str,
        total_text_len: usize,
        reasoning_len: usize,
        duration_ms: u128,
        ttft_ms: Option<u128>,
        finish_reason: Option<&str>,
        usage_opt: Option<&crate::types::Usage>,
    ) {
        let is_local = engine.contains("Local");
        let mut telemetry_lines = Vec::new();

        let ttft_str = if let Some(ttft) = ttft_ms {
            format!("{} ms", ttft)
        } else {
            "N/A".to_string()
        };

        telemetry_lines.push(format!("  • Engine:              {}", engine));
        telemetry_lines.push(format!("  • Model:               {}", model));
        telemetry_lines.push(format!("  • TTFT (First Token):  {}", ttft_str));
        telemetry_lines.push(format!("  • Total Duration:      {} ms", duration_ms));
        telemetry_lines.push(format!("  • Finish Reason:       {}", finish_reason.unwrap_or("stop")));
        telemetry_lines.push(format!("  • Output Characters:   {}", total_text_len));
        if reasoning_len > 0 {
            telemetry_lines.push(format!("  • Reasoning Length:    {} chars", reasoning_len));
        }

        let mut token_lines = Vec::new();
        if let Some(usage) = usage_opt {
            let prompt = usage.prompt_tokens;
            let cached = usage.prompt_cache_hit_tokens;
            let completion = usage.completion_tokens;
            let total = usage.total_tokens;
            let hit_rate = usage.cache_hit_percentage();

            let tps = if duration_ms > 0 && completion > 0 {
                (completion as f64) / (duration_ms as f64 / 1000.0)
            } else if duration_ms > 0 {
                ((total_text_len / 4) as f64) / (duration_ms as f64 / 1000.0)
            } else {
                0.0
            };
            telemetry_lines.push(format!("  • Output Speed:        {:.1} tokens/sec", tps));

            if is_local {
                token_lines.push(format!("  • Prompt Tokens:       {}", prompt));
                token_lines.push(format!("  • Completion Tokens:   {}", completion));
                token_lines.push(format!("  • Total Tokens:        {}", total));
                token_lines.push("  • Financial Cost:      $0.000000 USD (100% Free Local Hardware)".to_string());
                let cloud_saved = (prompt as f64 * (0.14 / 1_000_000.0)) + (completion as f64 * (0.28 / 1_000_000.0));
                token_lines.push(format!("  • Cloud Cost Saved:    ${:.6} USD", cloud_saved));
            } else {
                let miss = prompt.saturating_sub(cached);
                let hit_cost = cached as f64 * (0.014 / 1_000_000.0);
                let miss_cost = miss as f64 * (0.14 / 1_000_000.0);
                let completion_cost = completion as f64 * (0.28 / 1_000_000.0);
                let actual_cost = hit_cost + miss_cost + completion_cost;

                let un_cached_cost = (prompt as f64 * (0.14 / 1_000_000.0)) + completion_cost;
                let savings = un_cached_cost - actual_cost;
                let savings_pct = if un_cached_cost > 0.0 { (savings / un_cached_cost) * 100.0 } else { 0.0 };

                token_lines.push(format!("  • Prompt Tokens:       {} (Cached: {} · {:.1}% Hit Rate)", prompt, cached, hit_rate));
                token_lines.push(format!("  • Completion Tokens:   {}", completion));
                token_lines.push(format!("  • Total Tokens:        {}", total));
                token_lines.push(format!("  • Actual Cost:         ${:.6} USD", actual_cost));
                token_lines.push(format!("  • KV Cache Savings:    ${:.6} USD ({:.1}% discount)", savings, savings_pct));
            }
        } else {
            let tps = if duration_ms > 0 {
                ((total_text_len / 4) as f64) / (duration_ms as f64 / 1000.0)
            } else {
                0.0
            };
            telemetry_lines.push(format!("  • Approx Speed:        {:.1} tokens/sec", tps));
            if is_local {
                token_lines.push("  • Financial Cost:      $0.000000 USD (100% Free Local Hardware)".to_string());
            }
        }

        let details = format!(
            "Telemetry:\n{}\n\nToken & Cost Forensics:\n{}",
            telemetry_lines.join("\n"),
            token_lines.join("\n")
        );

        Self::log_event("LLM_RESP", &format!("INBOUND <- {} ({}) [{} ms]", engine, model, duration_ms), &details);
    }

    /// Logs a Tool Call Intent & Execution result with full stdout/stderr and duration
    pub fn log_tool_call(tool_name: &str, call_id: &str, arguments: &str, output: &str, duration_ms: u128, success: bool) {
        let status_str = if success { "SUCCESS" } else { "FAILED / ERROR" };
        let details = format!(
            "Tool:       {}\nCall ID:    {}\nStatus:     {}\nDuration:   {} ms\nArguments:\n{}\n\nExecution Output:\n{}",
            tool_name, call_id, status_str, duration_ms, arguments, output
        );
        Self::log_event("TOOL_EXEC", &format!("TOOL: {} [{}]", tool_name, status_str), &details);
    }

    /// Logs Hybrid Decision Router evaluation (e.g. why a turn went to Local vs Cloud)
    pub fn log_hybrid_decision(user_prompt: &str, is_coding_intent: bool, chosen_engine: &str, reason: &str) {
        let details = format!(
            "User Prompt:   {:?}\nIs Coding:     {}\nEngine Chosen: {}\nRationale:     {}",
            user_prompt, is_coding_intent, chosen_engine, reason
        );
        Self::log_event("HYBRID_ROUTE", &format!("DECISION -> {}", chosen_engine), &details);
    }

    /// Logs Any System / Network / Parse Errors
    pub fn log_error(context: &str, error_msg: &str) {
        let details = format!("Context: {}\nError:   {}", context, error_msg);
        Self::log_event("ERROR", &format!("FAILURE: {}", context), &details);
    }
}
