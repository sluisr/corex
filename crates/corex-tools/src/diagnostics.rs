//! Lightweight project diagnostics tool (`diagnostics`).
//!
//! Runs the workspace's native type-checker / compiler / linter (`cargo check`, `tsc --noEmit`,
//! `pyright`, `go vet`) and returns structured, concise diagnostics without forcing the model
//! to guess project-specific check commands or wade through verbose build output.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::types::{Tool, ToolContext, ToolOutput};

pub struct DiagnosticsTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticCommand {
    pub label: &'static str,
    pub program: &'static str,
    pub args: Vec<&'static str>,
}

/// Detects the most appropriate static diagnostic command for `workspace`.
pub fn detect_diagnostic_command(workspace: &Path, language: Option<&str>) -> Option<DiagnosticCommand> {
    if let Some(lang) = language.map(|s| s.trim().to_ascii_lowercase()) {
        match lang.as_str() {
            "rust" | "rs" | "cargo" => {
                return Some(DiagnosticCommand {
                    label: "cargo check",
                    program: "cargo",
                    args: vec!["check", "--message-format=short", "--all-targets"],
                });
            }
            "typescript" | "ts" | "tsx" => {
                return Some(DiagnosticCommand {
                    label: "tsc --noEmit",
                    program: "npx",
                    args: vec!["--no-install", "tsc", "--noEmit", "--pretty", "false"],
                });
            }
            "python" | "py" => {
                return Some(DiagnosticCommand {
                    label: "pyright",
                    program: "pyright",
                    args: vec!["--outputjson"],
                });
            }
            "go" | "golang" => {
                return Some(DiagnosticCommand {
                    label: "go vet",
                    program: "go",
                    args: vec!["vet", "./..."],
                });
            }
            _ => {}
        }
    }

    if workspace.join("Cargo.toml").exists() {
        Some(DiagnosticCommand {
            label: "cargo check",
            program: "cargo",
            args: vec!["check", "--message-format=short", "--all-targets"],
        })
    } else if workspace.join("tsconfig.json").exists() {
        Some(DiagnosticCommand {
            label: "tsc --noEmit",
            program: "npx",
            args: vec!["--no-install", "tsc", "--noEmit", "--pretty", "false"],
        })
    } else if workspace.join("pyrightconfig.json").exists() || workspace.join("pyproject.toml").exists() {
        Some(DiagnosticCommand {
            label: "pyright",
            program: "pyright",
            args: vec![],
        })
    } else if workspace.join("go.mod").exists() {
        Some(DiagnosticCommand {
            label: "go vet",
            program: "go",
            args: vec!["vet", "./..."],
        })
    } else {
        None
    }
}

#[async_trait]
impl Tool for DiagnosticsTool {
    fn name(&self) -> &str {
        "diagnostics"
    }

    fn description(&self) -> &str {
        "Runs the workspace's native type-checker / compiler diagnostics (Rust `cargo check`, TypeScript `tsc --noEmit`, Python `pyright`, Go `go vet`) and returns concise file:line:col errors and warnings."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "description": "Optional language override: rust, typescript, python, or go. Auto-detected from workspace files when omitted."
                },
                "filter_path": {
                    "type": "string",
                    "description": "Optional substring to filter diagnostics to a specific file or crate path."
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let language = args.get("language").and_then(|v| v.as_str());
        let filter = args
            .get("filter_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let Some(cmd_spec) = detect_diagnostic_command(&context.workspace_dir, language) else {
            return Ok(ToolOutput::error(
                "Could not detect a supported project toolchain (Cargo.toml, tsconfig.json, pyproject.toml/pyrightconfig.json, or go.mod).",
            ));
        };

        let mut cmd = tokio::process::Command::new(cmd_spec.program);
        cmd.args(&cmd_spec.args)
            .current_dir(&context.workspace_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let output = match tokio::time::timeout(Duration::from_secs(60), cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to run `{}`: {e}",
                    cmd_spec.label
                )));
            }
            Err(_) => {
                return Ok(ToolOutput::error(format!(
                    "`{}` timed out after 60s.",
                    cmd_spec.label
                )));
            }
        };

        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let lines: Vec<&str> = combined
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .filter(|l| filter.map_or(true, |f| l.contains(f)))
            .collect();

        if output.status.success() && lines.iter().all(|l| !l.contains("error") && !l.contains("warning")) {
            return Ok(ToolOutput::success(format!(
                "No diagnostics reported by `{}`.",
                cmd_spec.label
            )));
        }

        let capped: Vec<&str> = lines.into_iter().take(120).collect();
        let body = if capped.is_empty() {
            format!("`{}` exited with {:?} (no matching lines).", cmd_spec.label, output.status.code())
        } else {
            capped.join("\n")
        };
        Ok(ToolOutput::success(format!(
            "Diagnostics ({}):\n{}",
            cmd_spec.label, body
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_ts_python_and_go_projects() {
        let base = std::env::temp_dir().join(format!("corex-diag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let rs = base.join("rs");
        std::fs::create_dir_all(&rs).unwrap();
        std::fs::write(rs.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(detect_diagnostic_command(&rs, None).unwrap().label, "cargo check");

        let ts = base.join("ts");
        std::fs::create_dir_all(&ts).unwrap();
        std::fs::write(ts.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(detect_diagnostic_command(&ts, None).unwrap().label, "tsc --noEmit");

        let go = base.join("go");
        std::fs::create_dir_all(&go).unwrap();
        std::fs::write(go.join("go.mod"), "module x\n").unwrap();
        assert_eq!(detect_diagnostic_command(&go, None).unwrap().label, "go vet");

        let _ = std::fs::remove_dir_all(&base);
    }
}
