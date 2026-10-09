use std::borrow::Cow;
use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use diffy::Patch;

use crate::fs_tools::{atomic_write, lock_path, resolve_path};
use crate::types::{Tool, ToolContext, ToolOutput};

pub struct ApplyPatchTool;

impl ApplyPatchTool {
    /// Unified-diff lines are newline-terminated by definition, but a patch string can reach us
    /// without its final newline (hand-written patch, or a transport that trims trailing
    /// whitespace). `diffy` then fails on the last hunk, and that failure reads exactly like a
    /// *context* mismatch — sending the reader after a content difference that does not exist.
    /// Restore the terminator instead of failing on a cosmetic difference.
    fn normalize_patch(patch: &str) -> Cow<'_, str> {
        if patch.is_empty() || patch.ends_with('\n') {
            Cow::Borrowed(patch)
        } else {
            Cow::Owned(format!("{}\n", patch))
        }
    }

    /// Applies `patch` to `original`, tolerating two differences that are cosmetic to a human but
    /// fatal to a line-oriented matcher: a missing terminator on the last line, and CRLF endings.
    ///
    /// A hand-written patch rarely carries the `\ No newline at end of file` marker, and a patch
    /// payload cannot reliably transport a `\r` at all, so either way the file is effectively
    /// un-editable. Match against a normalized view of the file, then restore the file's own
    /// convention on the way out — the patch was not asked to rewrite endings it never mentioned.
    fn apply_tolerating_format_differences(
        original: &str,
        patch: &Patch<'_, str>,
    ) -> Result<String, String> {
        let (source, used_crlf) = Self::normalize_line_endings(original);

        let lacks_final_newline = !source.is_empty() && !source.ends_with('\n');
        let terminated = if lacks_final_newline {
            Cow::Owned(format!("{}\n", source))
        } else {
            Cow::Borrowed(source.as_ref())
        };

        let applied = diffy::apply(terminated.as_ref(), patch).map_err(|e| e.to_string())?;
        let mut result = if used_crlf {
            applied.replace('\n', "\r\n")
        } else {
            applied
        };

        if lacks_final_newline && result.ends_with('\n') {
            result.pop();
        }

        Ok(result)
    }

    /// Returns a LF view of `original` when its endings are uniformly CRLF, plus whether that
    /// conversion happened. Uniformity is required: rewriting a file that mixes endings would
    /// change bytes the patch never mentioned, so mixed files keep the strict, loud behaviour.
    fn normalize_line_endings(original: &str) -> (Cow<'_, str>, bool) {
        let newlines = original.matches('\n').count();
        let crlf = original.matches("\r\n").count();

        if newlines > 0 && newlines == crlf {
            (Cow::Owned(original.replace("\r\n", "\n")), true)
        } else {
            (Cow::Borrowed(original), false)
        }
    }

    fn extract_file_path(hunk_header: &str) -> Option<String> {
        // Priority 1: +++ b/path (post-image target)
        for line in hunk_header.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("+++ ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let mut file = parts[1].trim();
                    if file.starts_with("b/") || file.starts_with("a/") {
                        file = &file[2..];
                    }
                    if file != "/dev/null" && !file.is_empty() {
                        return Some(file.to_string());
                    }
                }
            }
        }
        // Priority 2: --- a/path (pre-image fallback)
        for line in hunk_header.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("--- ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let mut file = parts[1].trim();
                    if file.starts_with("a/") || file.starts_with("b/") {
                        file = &file[2..];
                    }
                    if file != "/dev/null" && !file.is_empty() {
                        return Some(file.to_string());
                    }
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
struct ParsedHunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<(char, String)>,
}

fn parse_hunk_starts(header: &str) -> Option<(usize, usize)> {
    let old_start = if let Some(start_pos) = header.find('-') {
        let rest = &header[start_pos + 1..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        num_str.parse::<usize>().ok()?
    } else {
        return None;
    };
    let new_start = if let Some(start_pos) = header.find('+') {
        let rest = &header[start_pos + 1..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        num_str.parse::<usize>().ok()?
    } else {
        return None;
    };
    Some((old_start, new_start))
}

/// Sanitizes LLM-generated patch inputs:
/// - Strips markdown code blocks (```diff ... ```)
/// - Strips prose preambles before the diff
/// - Adds missing leading space to context lines
/// - Recomputes exact hunk line counts in @@ headers so diffy never rejects count mismatches
fn sanitize_and_repair_patch(raw: &str) -> (String, Vec<ParsedHunk>) {
    let mut s = raw.trim();
    if s.starts_with("```") {
        if let Some(pos) = s.find('\n') {
            s = &s[pos + 1..];
        }
    }
    if let Some(pos) = s.rfind("```") {
        s = &s[..pos];
    }
    let s = s.trim();

    let raw_lines: Vec<&str> = s.lines().collect();
    let start_idx = raw_lines
        .iter()
        .position(|l| {
            let t = l.trim_start();
            t.starts_with("--- ") || t.starts_with("+++ ") || t.starts_with("@@")
        })
        .unwrap_or(0);

    let mut header_lines = Vec::new();
    let mut hunks: Vec<ParsedHunk> = Vec::new();
    let mut current_hunk: Option<ParsedHunk> = None;
    let mut has_unsupported_hunk = false;

    for line in &raw_lines[start_idx..] {
        let trimmed_end = line.trim_end();
        let trimmed_start = trimmed_end.trim_start();

        if trimmed_start.starts_with("--- ") || trimmed_start.starts_with("+++ ") {
            if current_hunk.is_none() {
                header_lines.push(trimmed_start.to_string());
            }
            continue;
        }

        if trimmed_start.starts_with("@@") {
            if let Some(h) = current_hunk.take() {
                hunks.push(h);
            }
            if let Some((old_start, new_start)) = parse_hunk_starts(trimmed_start) {
                current_hunk = Some(ParsedHunk {
                    old_start,
                    old_count: 0,
                    new_start,
                    new_count: 0,
                    lines: Vec::new(),
                });
            } else {
                // Bare @@ without line numbers - leave as no-op / unsupported
                has_unsupported_hunk = true;
            }
            continue;
        }

        if let Some(ref mut hunk) = current_hunk {
            if trimmed_start.starts_with("```") {
                break;
            }
            if trimmed_end.is_empty() {
                hunk.lines.push((' ', String::new()));
                continue;
            }
            let first_char = trimmed_end.chars().next().unwrap_or(' ');
            match first_char {
                '+' | '-' | ' ' => {
                    hunk.lines.push((first_char, trimmed_end[1..].to_string()));
                }
                '\\' => {
                    // Ignore "\ No newline at end of file"
                }
                _ => {
                    // Model forgot leading space for context line
                    hunk.lines.push((' ', trimmed_end.to_string()));
                }
            }
        }
    }

    if let Some(h) = current_hunk {
        hunks.push(h);
    }

    if has_unsupported_hunk && hunks.is_empty() {
        // Return raw patch untouched so diffy's no-op behavior applies and rejection triggers
        return (raw.to_string(), Vec::new());
    }

    let mut repaired_diff = String::new();
    for h in &header_lines {
        repaired_diff.push_str(h);
        repaired_diff.push('\n');
    }

    for hunk in &mut hunks {
        hunk.old_count = hunk.lines.iter().filter(|(c, _)| *c == ' ' || *c == '-').count();
        hunk.new_count = hunk.lines.iter().filter(|(c, _)| *c == ' ' || *c == '+').count();

        repaired_diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));
        for (c, l) in &hunk.lines {
            repaired_diff.push(*c);
            repaired_diff.push_str(l);
            repaired_diff.push('\n');
        }
    }

    (repaired_diff, hunks)
}


fn apply_fuzzy_hunks(original: &str, hunks: &[ParsedHunk]) -> Result<String, String> {
    if hunks.is_empty() {
        return Err("No hunks found in patch".to_string());
    }

    let (source, used_crlf) = ApplyPatchTool::normalize_line_endings(original);
    let lacks_final_newline = !source.is_empty() && !source.ends_with('\n');

    let mut file_lines: Vec<String> = source.lines().map(|s| s.to_string()).collect();
    let mut any_applied = false;

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        let old_lines: Vec<&str> = hunk
            .lines
            .iter()
            .filter(|(c, _)| *c == ' ' || *c == '-')
            .map(|(_, l)| l.as_str())
            .collect();

        let new_lines: Vec<String> = hunk
            .lines
            .iter()
            .filter(|(c, _)| *c == ' ' || *c == '+')
            .map(|(_, l)| l.clone())
            .collect();

        if old_lines.is_empty() {
            let pos = hunk.new_start.saturating_sub(1).min(file_lines.len());
            for (idx, nl) in new_lines.into_iter().enumerate() {
                file_lines.insert(pos + idx, nl);
            }
            any_applied = true;
            continue;
        }

        let match_range = find_hunk_match(&file_lines, &old_lines, hunk.old_start);
        if let Some((start_idx, end_idx)) = match_range {
            file_lines.splice(start_idx..end_idx, new_lines);
            any_applied = true;
        } else {
            return Err(format!(
                "Hunk #{} failed to match file content near line {} (target lines: {}). Context does not match.",
                hunk_idx + 1,
                hunk.old_start,
                old_lines.len()
            ));
        }
    }

    if !any_applied {
        return Err("No hunks could be matched or applied".to_string());
    }

    let mut result = file_lines.join("\n");
    if !lacks_final_newline {
        result.push('\n');
    }
    if used_crlf {
        result = result.replace('\n', "\r\n");
    }

    Ok(result)
}

fn find_hunk_match(file_lines: &[String], old_lines: &[&str], hint_line: usize) -> Option<(usize, usize)> {
    if old_lines.is_empty() || old_lines.len() > file_lines.len() {
        return None;
    }

    let target_len = old_lines.len();
    let hint_idx = hint_line
        .saturating_sub(1)
        .min(file_lines.len().saturating_sub(target_len));

    let mut candidates = Vec::with_capacity(file_lines.len() - target_len + 1);
    candidates.push(hint_idx);
    let mut offset = 1;
    while hint_idx >= offset || hint_idx + offset <= file_lines.len().saturating_sub(target_len) {
        if hint_idx + offset <= file_lines.len().saturating_sub(target_len) {
            candidates.push(hint_idx + offset);
        }
        if hint_idx >= offset {
            candidates.push(hint_idx - offset);
        }
        offset += 1;
    }

    // Pass 1: Exact equality
    for &i in &candidates {
        if file_lines[i..i + target_len]
            .iter()
            .zip(old_lines.iter())
            .all(|(f, o)| f == *o)
        {
            return Some((i, i + target_len));
        }
    }

    // Pass 2: Ignore trailing whitespace
    for &i in &candidates {
        if file_lines[i..i + target_len]
            .iter()
            .zip(old_lines.iter())
            .all(|(f, o)| f.trim_end() == o.trim_end())
        {
            return Some((i, i + target_len));
        }
    }

    // Pass 3: Ignore indentation / full whitespace trim
    for &i in &candidates {
        if file_lines[i..i + target_len]
            .iter()
            .zip(old_lines.iter())
            .all(|(f, o)| f.trim() == o.trim())
        {
            return Some((i, i + target_len));
        }
    }

    // Pass 4: Fuzz 1 - ignore first and last context line if hunk has >= 3 lines
    if target_len >= 3 {
        let sub_old = &old_lines[1..target_len - 1];
        let sub_len = sub_old.len();
        for &i in &candidates {
            if i + 1 + sub_len <= file_lines.len() {
                if file_lines[i + 1..i + 1 + sub_len]
                    .iter()
                    .zip(sub_old.iter())
                    .all(|(f, o)| f.trim() == o.trim())
                {
                    return Some((i, i + target_len));
                }
            }
        }
    }

    None
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn description(&self) -> &'static str {
        "Applies unified diff patches directly to files. Fast, atomic, and token-efficient for code editing. [PREFERRED for code edits]"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "The unified diff patch content to apply."
                },
                "file_path": {
                    "type": "string",
                    "description": "Optional: Path of the file to patch if not specified in diff headers (supports ~/)."
                }
            },
            "required": ["patch"]
        })
    }

    fn needs_confirmation(&self, _args: &serde_json::Value, context: &ToolContext) -> bool {
        !context.allowed_commands.iter().any(|cmd| cmd == "apply_patch")
    }

    fn format_diff(&self, args: &serde_json::Value, _workspace: &Path) -> Option<String> {
        args.get("patch")
            .or_else(|| args.get("input"))
            .or_else(|| args.get("diff"))
            .and_then(|p| p.as_str())
            .or_else(|| args.as_str())
            .map(|s| s.to_string())
    }

    async fn execute(&self, args: serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let patch_str = match args.get("patch")
            .or_else(|| args.get("input"))
            .or_else(|| args.get("diff"))
            .and_then(|v| v.as_str())
            .or_else(|| args.as_str())
        {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(ToolOutput::error("Error: 'patch' parameter cannot be empty.")),
        };

        // Extract target file path before repair or from raw patch headers
        let target_file = match Self::extract_file_path(patch_str) {
            Some(f) => f,
            None => match args.get("file_path")
                .or_else(|| args.get("path"))
                .or_else(|| args.get("file"))
                .and_then(|v| v.as_str())
            {
                Some(f) => f.to_string(),
                None => {
                    return Ok(ToolOutput::error(
                        "Error: Could not determine target file from diff headers (e.g. '+++ b/path/to/file') and 'file_path' was not provided."
                    ));
                }
            },
        };

        let target_path = resolve_path(&context.workspace_dir, &target_file);

        // Hold the per-path lock across the whole read-modify-write
        let _guard = lock_path(&target_path).await;

        let original_content = if target_path.exists() {
            match fs::read_to_string(&target_path) {
                Ok(c) => c,
                Err(e) => return Ok(ToolOutput::error(format!("Failed to read target file {}: {}", target_path.display(), e))),
            }
        } else {
            String::new()
        };

        // 1. Sanitize, unwrap markdown fences, normalize context lines, and repair hunk counts
        let (repaired_patch, hunks) = sanitize_and_repair_patch(patch_str);
        let normalized_patch = Self::normalize_patch(&repaired_patch);

        // 2. Attempt primary application via diffy
        let mut patch_result = None;
        if let Ok(diffy_patch) = Patch::from_str(normalized_patch.as_ref()) {
            if let Ok(res) = Self::apply_tolerating_format_differences(&original_content, &diffy_patch) {
                if res != original_content {
                    patch_result = Some(res);
                }
            }
        }

        // 3. Fallback: Fuzzy hunk matching if diffy failed or yielded no changes
        if patch_result.is_none() && !hunks.is_empty() {
            match apply_fuzzy_hunks(&original_content, &hunks) {
                Ok(res) if res != original_content => {
                    patch_result = Some(res);
                }
                Err(fuzzy_err) => {
                    tracing::debug!("apply_patch fuzzy fallback failed: {}", fuzzy_err);
                }
                _ => {}
            }
        }

        let patched_content = match patch_result {
            Some(res) => res,
            None => {
                return Ok(ToolOutput::error(format!(
                    "Patch application failed for {}. The hunk context does not match the current file \
                     content or produced no change. Please read the file again and provide updated context.",
                    target_file
                )));
            }
        };

        // Compare-and-swap: refuse to clobber a file that changed under us
        if let Ok(current) = fs::read_to_string(&target_path) {
            if current != original_content {
                return Ok(ToolOutput::error(format!(
                    "Refusing to patch {}: the file changed on disk after it was read. Re-read it and retry.",
                    target_file
                )));
            }
        }

        if let Err(e) = atomic_write(&target_path, &patched_content) {
            return Ok(ToolOutput::error(format!("Failed to write patched content to {}: {}", target_path.display(), e)));
        }

        let action = if original_content.is_empty() { "Created" } else { "Patched" };
        Ok(ToolOutput::success_with_summary(
            format!("{} {} successfully via apply_patch.", action, target_file),
            format!("{} {}", action, target_file),
        ))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A file whose last line has no terminator must be patchable without the author knowing the
    /// `\ No newline at end of file` marker — and must stay un-terminated afterwards.
    #[test]
    fn test_patch_applies_to_file_without_final_newline() {
        let original = "uno\ndos\ntres";
        let patch = Patch::from_str("@@ -1,3 +1,3 @@\n uno\n-dos\n+DOS\n tres\n").unwrap();

        let applied = ApplyPatchTool::apply_tolerating_format_differences(original, &patch)
            .expect("must apply without the marker");
        assert_eq!(applied, "uno\nDOS\ntres", "the missing terminator must be preserved");
    }

    /// The tolerance must not alter files that were already well-formed.
    #[test]
    fn test_terminated_file_is_unaffected_by_the_tolerance() {
        let original = "uno\ndos\ntres\n";
        let patch = Patch::from_str("@@ -1,3 +1,3 @@\n uno\n-dos\n+DOS\n tres\n").unwrap();

        let applied = ApplyPatchTool::apply_tolerating_format_differences(original, &patch)
            .expect("must apply");
        assert_eq!(applied, "uno\nDOS\ntres\n", "a terminated file must stay terminated");
    }

    #[test]
    fn test_patch_missing_final_newline_is_normalized() {
        let original = "uno\ndos\ntres\n";
        let raw = "@@ -1,3 +1,3 @@\n uno\n-dos\n+DOS_1\n tres";

        assert!(!raw.ends_with('\n'), "fixture must reproduce the missing terminator");

        let normalized = ApplyPatchTool::normalize_patch(raw);
        let patch = Patch::from_str(&normalized).expect("normalized patch must parse");
        let applied = diffy::apply(original, &patch).expect("normalized patch must apply");
        assert_eq!(applied, "uno\nDOS_1\ntres\n");
    }

    /// Documents *why* normalization exists: raw, the very same patch is rejected.
    #[test]
    fn test_raw_patch_without_final_newline_fails_to_apply() {
        let original = "uno\ndos\ntres\n";
        let raw = "@@ -1,3 +1,3 @@\n uno\n-dos\n+DOS_1\n tres";

        let patch = Patch::from_str(raw).expect("it does still parse");
        assert!(
            diffy::apply(original, &patch).is_err(),
            "if this ever passes, normalization is redundant and can be removed"
        );
    }

    /// A correctly terminated patch must not be copied.
    #[test]
    fn test_normalize_patch_leaves_terminated_patch_untouched() {
        let well_formed = "@@ -1 +1 @@\n-a\n+b\n";
        assert!(matches!(
            ApplyPatchTool::normalize_patch(well_formed),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn test_format_diff_flexible_keys() {
        let tool = ApplyPatchTool;
        let ws = Path::new("/tmp");

        let args_patch = json!({"patch": "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b\n"});
        assert!(tool.format_diff(&args_patch, ws).is_some());

        let args_input = json!({"input": "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b\n"});
        assert!(tool.format_diff(&args_input, ws).is_some());

        let args_diff = json!({"diff": "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b\n"});
        assert!(tool.format_diff(&args_diff, ws).is_some());
    }

    /// End-to-end through `execute`: read, apply, compare-and-swap and atomic write, against a file
    /// whose last line has no terminator, using a patch that carries no marker for it.
    #[tokio::test]
    async fn test_execute_patches_file_without_final_newline() {
        let dir = std::env::temp_dir().join(format!(
            "corex_patch_e2e_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();

        let target = dir.join("notrail.txt");
        fs::write(&target, "uno\ndos\ntres").unwrap(); // deliberately unterminated

        let patch =
            "--- a/notrail.txt\n+++ b/notrail.txt\n@@ -1,3 +1,3 @@\n uno\n-dos\n+DOS\n tres\n";
        assert!(
            !patch.contains("No newline"),
            "fixture must carry no marker, or it proves nothing"
        );

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let out = ApplyPatchTool
            .execute(json!({ "patch": patch }), &context)
            .await
            .expect("execute must not bail");

        assert!(!out.is_error, "patch should have applied, got: {}", out.output);

        let bytes = fs::read(&target).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            "uno\nDOS\ntres",
            "the file's missing terminator must survive the round trip"
        );
        assert_eq!(*bytes.last().unwrap(), b's', "no newline may be appended");

        // The negative control: without the tolerance this very patch is rejected, so this test
        // genuinely guards the behaviour instead of passing either way.
        assert!(
            diffy::apply("uno\ndos\ntres", &Patch::from_str(patch).unwrap()).is_err(),
            "if this ever passes, the tolerance is redundant and can be removed"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "corex_patch_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `diffy` itself accepts a hunk header with no line numbers (`@@`) as a hunk that matches
    /// nothing and changes nothing. This documents *why* `execute` refuses a change-less result, so
    /// the guard is not later dismissed as redundant.
    #[test]
    fn test_diffy_treats_numberless_hunk_header_as_a_no_op() {
        let original = "uno\ndos\ntres\n";
        let patch = Patch::from_str("@@\n-dos\n+DOS\n").expect("diffy does parse it");

        assert_eq!(
            diffy::apply(original, &patch).unwrap(),
            original,
            "if this ever differs, revisit the change-less guard in execute"
        );
    }

    /// End-to-end: a patch that changes nothing must be reported as a failure. Reporting success
    /// here is the worst outcome available — the caller believes the edit landed and stops looking.
    #[tokio::test]
    async fn test_execute_rejects_patch_that_changes_nothing() {
        let dir = unique_temp_dir("noop");
        let target = dir.join("notrail.txt");
        fs::write(&target, "uno\ndos\ntres").unwrap();

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let out = ApplyPatchTool
            .execute(
                json!({ "patch": "@@\n-dos\n+DOS\n", "file_path": "notrail.txt" }),
                &context,
            )
            .await
            .expect("execute must not bail");

        assert!(
            out.is_error,
            "a no-op patch must not be reported as success, got: {}",
            out.output
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "uno\ndos\ntres",
            "the file must be left untouched"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    /// A uniformly-CRLF file must be patchable even though the patch payload cannot carry a `\r`,
    /// and must come out CRLF on every line — not silently converted to LF.
    #[test]
    fn test_patch_applies_to_crlf_file_and_preserves_endings() {
        let original = "alfa\r\nbeta\r\ngamma\r\n";
        let patch = Patch::from_str("@@ -1,3 +1,3 @@\n alfa\n-beta\n+BETA\n gamma\n").unwrap();

        let applied = ApplyPatchTool::apply_tolerating_format_differences(original, &patch)
            .expect("a CRLF file must be patchable without transporting a CR");

        assert_eq!(applied, "alfa\r\nBETA\r\ngamma\r\n");
        assert_eq!(
            applied.matches('\n').count(),
            applied.matches("\r\n").count(),
            "every line must still be CRLF"
        );
    }

    /// Documents *why* the CRLF tolerance exists: the same patch with LF endings does not match,
    /// because the matcher compares raw lines and the file's carry a `\r`.
    #[test]
    fn test_lf_context_does_not_match_a_crlf_file_without_the_tolerance() {
        let original = "alfa\r\nbeta\r\ngamma\r\n";
        let patch = Patch::from_str("@@ -1,3 +1,3 @@\n alfa\n-beta\n+BETA\n gamma\n").unwrap();

        assert!(
            diffy::apply(original, &patch).is_err(),
            "if this ever passes, the CRLF tolerance is redundant and can be removed"
        );
    }

    /// A file mixing endings must keep the strict behaviour: normalizing it would rewrite bytes the
    /// patch never mentioned.
    #[test]
    fn test_mixed_endings_are_not_normalized() {
        let (view, converted) = ApplyPatchTool::normalize_line_endings("uno\r\ndos\ntres\n");
        assert!(!converted, "a mixed file must not be rewritten");
        assert_eq!(view.as_ref(), "uno\r\ndos\ntres\n");
    }

    #[tokio::test]
    async fn test_execute_handles_markdown_fences_and_inaccurate_counts() {
        let dir = unique_temp_dir("md_test");
        let target = dir.join("code.rs");
        fs::write(&target, "fn foo() {\n    let a = 1;\n    let b = 2;\n    println!(\"{}\", a + b);\n}\n").unwrap();

        // Model generated markdown fences, wrong count in @@ header (claims 99 lines),
        // and context lines with missing leading space!
        let messy_patch = r#"Here is the diff:
```diff
--- a/code.rs
+++ b/code.rs
@@ -1,99 +1,99 @@
fn foo() {
-    let a = 1;
+    let a = 100;
     let b = 2;
     println!("{}", a + b);
}
```
Hope this helps!
"#;

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let out = ApplyPatchTool
            .execute(json!({ "patch": messy_patch, "file_path": "code.rs" }), &context)
            .await
            .expect("execute must succeed");

        assert!(!out.is_error, "patch must succeed despite messy LLM formatting: {}", out.output);
        let updated = fs::read_to_string(&target).unwrap();
        assert!(updated.contains("let a = 100;"));
        assert!(!updated.contains("let a = 1;"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_execute_handles_fuzzy_whitespace_matching() {
        let dir = unique_temp_dir("fuzzy_ws");
        let target = dir.join("code.py");
        // Target file has trailing spaces on some lines
        fs::write(&target, "def hello():   \n    x = 10   \n    return x\n").unwrap();

        // Patch was written without trailing whitespace
        let patch = "--- a/code.py\n+++ b/code.py\n@@ -1,3 +1,3 @@\n def hello():\n-    x = 10\n+    x = 20\n return x\n";

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
        };

        let out = ApplyPatchTool
            .execute(json!({ "patch": patch, "file_path": "code.py" }), &context)
            .await
            .expect("execute must succeed");

        assert!(!out.is_error, "fuzzy matching should succeed: {}", out.output);
        let updated = fs::read_to_string(&target).unwrap();
        assert!(updated.contains("x = 20"));

        fs::remove_dir_all(&dir).unwrap();
    }
}


