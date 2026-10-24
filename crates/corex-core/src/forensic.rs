use std::fs;
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
    /// Initializes the global session logger under `~/.corex/logs/corex-forensic-YYYY-MM-DD.log`.
    pub fn init(workspace_dir: Option<&Path>) -> PathBuf {
        let date_str = Local::now().format("%Y-%m-%d").to_string();
        let log_dir = if let Some(dirs) = BaseDirs::new() {
            dirs.home_dir().join(".corex").join("logs")
        } else {
            PathBuf::from(".corex").join("logs")
        };

        let _ = crate::secure_fs::ensure_private_dir(&log_dir);
        let log_file_path = log_dir.join(format!("corex-forensic-{}.log", date_str));

        // Purge logs older than 7 days.
        if let Ok(entries) = fs::read_dir(&log_dir) {
            let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 86400);
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("corex-forensic-") && name.ends_with(".log") {
                        if let Ok(meta) = entry.metadata() {
                            if let Ok(modified) = meta.modified() {
                                if modified < cutoff {
                                    let _ = fs::remove_file(&p);
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut global = match LOGGER.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        *global = Some(ForensicLogger {
            log_file_path: log_file_path.clone(),
        });

        Self::raw_append(
            &log_file_path,
            &format!(
                "\n--- SESSION START {} | workspace={} ---\n",
                Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                workspace_dir
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "N/A".to_string())
            ),
        );

        log_file_path
    }

    fn raw_append(path: &Path, content: &str) {
        // Cap single-day log files at 20 MiB.
        if let Ok(meta) = fs::metadata(path) {
            if meta.len() > 20 * 1024 * 1024 {
                return;
            }
        }
        if let Ok(mut file) = crate::secure_fs::open_private(path, true) {
            let _ = file.write_all(content.as_bytes());
            let _ = file.flush();
        }
    }

    pub fn is_enabled() -> bool {
        LOGGER.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn log_event(category: &str, title: &str, details: &str) {
        let log_path = match LOGGER.lock() {
            Ok(guard) => guard.as_ref().map(|l| l.log_file_path.clone()),
            Err(_) => None,
        };
        let Some(path) = log_path else {
            return;
        };

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let formatted = format!(
            "\n[{}][{:<12}] --- {} ---\n{}\n{}\n",
            timestamp,
            category.to_uppercase(),
            title,
            details.trim(),
            "-".repeat(80)
        );

        Self::raw_append(&path, &formatted);
    }

    pub fn log_llm_request(engine: &str, model: &str, endpoint: &str, messages_count: usize, tools_count: usize, body: &str) {
        if !Self::is_enabled() {
            return;
        }
        let body_preview = if body.len() > 8_192 {
            format!(
                "{}\n... [{} bytes truncated]",
                crate::types::safe_truncate_str(body, 8_192),
                body.len() - 8_192
            )
        } else {
            body.to_string()
        };
        let details = format!(
            "Engine:     {}\nModel:      {}\nEndpoint:   {}\nMessages:   {}\nTools:      {}\nPayload (Preview):\n{}",
            engine, model, endpoint, messages_count, tools_count, body_preview
        );
        Self::log_event("LLM_REQ", &format!("OUTBOUND -> {} ({})", engine, model), &details);
    }

    #[allow(clippy::too_many_arguments)]
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

        telemetry_lines.push(format!("  Engine:              {}", engine));
        telemetry_lines.push(format!("  Model:               {}", model));
        telemetry_lines.push(format!("  TTFT:                {}", ttft_str));
        telemetry_lines.push(format!("  Duration:            {} ms", duration_ms));
        telemetry_lines.push(format!("  Finish Reason:       {}", finish_reason.unwrap_or("stop")));
        telemetry_lines.push(format!("  Output Characters:   {}", total_text_len));
        if reasoning_len > 0 {
            telemetry_lines.push(format!("  Reasoning Length:    {} chars", reasoning_len));
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
            telemetry_lines.push(format!("  Throughput:          {:.1} tok/s", tps));

            if is_local {
                token_lines.push(format!("  Prompt Tokens:       {}", prompt));
                token_lines.push(format!("  Completion Tokens:   {}", completion));
                token_lines.push(format!("  Total Tokens:        {}", total));
                token_lines.push("  Cost:                $0.000000 USD (local)".to_string());
                let cloud_saved = (prompt as f64 * (0.14 / 1_000_000.0)) + (completion as f64 * (0.28 / 1_000_000.0));
                token_lines.push(format!("  Cloud Equivalent:    ${:.6} USD", cloud_saved));
            } else {
                let miss = prompt.saturating_sub(cached);
                let hit_cost = cached as f64 * (0.014 / 1_000_000.0);
                let miss_cost = miss as f64 * (0.14 / 1_000_000.0);
                let completion_cost = completion as f64 * (0.28 / 1_000_000.0);
                let actual_cost = hit_cost + miss_cost + completion_cost;

                let un_cached_cost = (prompt as f64 * (0.14 / 1_000_000.0)) + completion_cost;
                let savings = un_cached_cost - actual_cost;
                let savings_pct = if un_cached_cost > 0.0 { (savings / un_cached_cost) * 100.0 } else { 0.0 };

                token_lines.push(format!("  Prompt Tokens:       {} (cached: {} · {:.1}%)", prompt, cached, hit_rate));
                token_lines.push(format!("  Completion Tokens:   {}", completion));
                token_lines.push(format!("  Total Tokens:        {}", total));
                token_lines.push(format!("  Cost:                ${:.6} USD", actual_cost));
                token_lines.push(format!("  Cache Savings:       ${:.6} USD ({:.1}%)", savings, savings_pct));
            }
        } else {
            let tps = if duration_ms > 0 {
                ((total_text_len / 4) as f64) / (duration_ms as f64 / 1000.0)
            } else {
                0.0
            };
            telemetry_lines.push(format!("  Approx Throughput:   {:.1} tok/s", tps));
            if is_local {
                token_lines.push("  Cost:                $0.000000 USD (local)".to_string());
            }
        }

        let details = format!(
            "Telemetry:\n{}\n\nUsage:\n{}",
            telemetry_lines.join("\n"),
            token_lines.join("\n")
        );

        Self::log_event("LLM_RESP", &format!("INBOUND <- {} ({}) [{} ms]", engine, model, duration_ms), &details);
    }

    pub fn log_tool_call(tool_name: &str, call_id: &str, arguments: &str, output: &str, duration_ms: u128, success: bool) {
        if !Self::is_enabled() {
            return;
        }
        let status_str = if success { "SUCCESS" } else { "FAILED" };
        let capped_output = if output.len() > 4_096 {
            format!(
                "{}\n... [{} bytes truncated]",
                crate::types::safe_truncate_str(output, 4_096),
                output.len() - 4_096
            )
        } else {
            output.to_string()
        };
        let details = format!(
            "Tool:       {}\nCall ID:    {}\nStatus:     {}\nDuration:   {} ms\nArguments:\n{}\n\nOutput:\n{}",
            tool_name, call_id, status_str, duration_ms, arguments, capped_output
        );
        Self::log_event("TOOL_EXEC", &format!("TOOL: {} [{}]", tool_name, status_str), &details);
    }

    pub fn log_error(context: &str, error_msg: &str) {
        let details = format!("Context: {}\nError:   {}", context, error_msg);
        Self::log_event("ERROR", &format!("FAILURE: {}", context), &details);
    }

    pub fn get_subagents_log_dir() -> PathBuf {
        let base = if let Some(dirs) = BaseDirs::new() {
            dirs.home_dir().join(".corex").join("logs").join("subagents")
        } else {
            PathBuf::from(".corex").join("logs").join("subagents")
        };
        let _ = crate::secure_fs::ensure_private_dir(&base);
        base
    }

    #[allow(clippy::too_many_arguments)]
    pub fn write_subagent_forensic_log(
        task_id: &str,
        role: &str,
        status: &str,
        started_at: &str,
        finished_at: &str,
        duration_str: &str,
        workspace: &str,
        prompt: &str,
        steps: &[String],
        final_output: &str,
    ) -> PathBuf {
        let dir = Self::get_subagents_log_dir();
        let safe_task_id: String = task_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let effective_id = if safe_task_id.is_empty() { "subagent" } else { &safe_task_id };
        let log_file = dir.join(format!("{}.log", effective_id));

        let mut content = String::new();
        content.push_str("--- SUBAGENT EXECUTION LOG ---\n");
        content.push_str(&format!("Task ID:   {}\n", task_id));
        content.push_str(&format!("Role:      {}\n", role.to_uppercase()));
        content.push_str(&format!("Status:    {}\n", status.to_uppercase()));
        content.push_str(&format!("Workspace: {}\n", workspace));
        content.push_str(&format!("Started:   {}\n", started_at));
        content.push_str(&format!("Finished:  {}\n", finished_at));
        content.push_str(&format!("Duration:  {}\n\n", duration_str));

        content.push_str("--- PROMPT ---\n");
        content.push_str(prompt.trim());
        content.push_str("\n\n");

        content.push_str("--- STEPS ---\n");
        if steps.is_empty() {
            content.push_str("(none)\n");
        } else {
            for (idx, step) in steps.iter().enumerate() {
                content.push_str(&format!("[{:02}] {}\n", idx + 1, step));
            }
        }
        content.push_str("\n");

        content.push_str("--- OUTPUT ---\n");
        content.push_str(final_output.trim());
        content.push_str("\n");

        if let Ok(mut file) = crate::secure_fs::open_private(&log_file, true) {
            let _ = file.write_all(content.as_bytes());
            let _ = file.flush();
        }

        let details = format!(
            "Task ID:   {}\nRole:      {}\nStatus:    {}\nDuration:  {}\nLog File:  {}\nPrompt:    {}\nSteps:     {}\nOutput:\n{}",
            task_id, role, status, duration_str, log_file.display(), prompt, steps.len(), final_output
        );
        Self::log_event("SUBAGENT", &format!("SUBAGENT: {} [{}] ({})", role, task_id, status), &details);

        log_file
    }
}
