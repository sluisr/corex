use std::borrow::Cow;
use std::fs;
use std::path::Path;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use diffy::Patch;

use crate::fs_tools::{atomic_write, check_observed, lock_path, observe_file, resolve_path};
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
            if used_crlf && result.ends_with('\r') {
                result.pop();
            }
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

    /// True when the post-image of the patch is `/dev/null`, i.e. the patch removes the file.
    fn deletes_target(patch: &str) -> bool {
        patch
            .lines()
            .take_while(|l| !l.trim_start().starts_with("@@"))
            .any(|l| {
                let t = l.trim();
                (t.starts_with("+++ ") || t.starts_with("+++\t"))
                    && t[3..].trim().split('\t').next() == Some("/dev/null")
            })
    }

    pub(crate) fn extract_file_path(hunk_header: &str) -> Option<String> {
        if let Some(path) = codex_target(hunk_header) {
            return Some(path.1);
        }
        let clean_path = |file: &str| -> Option<String> {
            let trimmed = file.trim().trim_matches('"').trim();
            let mut f = trimmed;
            if f.starts_with("b/") || f.starts_with("a/") {
                f = &f[2..];
            }
            // Strip any multiple leading slashes (e.g. from `a//home/...`)
            while f.starts_with("//") {
                f = &f[1..];
            }
            if f != "/dev/null" && !f.is_empty() {
                Some(f.to_string())
            } else {
                None
            }
        };

        let header_section: Vec<&str> = hunk_header
            .lines()
            .take_while(|l| !l.trim_start().starts_with("@@"))
            .collect();

        // The post-image path wins, then the pre-image one, then the `diff --git` header.
        for line in &header_section {
            let trimmed = line.trim();
            if (trimmed.starts_with("+++ ") || trimmed.starts_with("+++\t")) && !trimmed.starts_with("++++") {
                let rest = trimmed.strip_prefix("+++").unwrap_or("").trim_start();
                let path_part = rest.split('\t').next().unwrap_or("").trim();
                if let Some(res) = clean_path(path_part) {
                    return Some(res);
                }
            }
        }
        for line in &header_section {
            let trimmed = line.trim();
            if (trimmed.starts_with("--- ") || trimmed.starts_with("---\t")) && !trimmed.starts_with("----") {
                let rest = trimmed.strip_prefix("---").unwrap_or("").trim_start();
                let path_part = rest.split('\t').next().unwrap_or("").trim();
                if let Some(res) = clean_path(path_part) {
                    return Some(res);
                }
            }
        }
        for line in &header_section {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("diff --git ") {
                if let Some((_, b_part)) = rest.rsplit_once(" b/") {
                    if let Some(res) = clean_path(b_part) {
                        return Some(res);
                    }
                } else {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 4 {
                        if let Some(res) = clean_path(parts[3]) {
                            return Some(res);
                        }
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
    let start_pos = header.find('-')?;
    let rest = &header[start_pos + 1..];
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let old_start = num_str.parse::<usize>().ok()?;

    let start_pos = header.find('+')?;
    let rest = &header[start_pos + 1..];
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let new_start = num_str.parse::<usize>().ok()?;

    Some((old_start, new_start))
}

/// Sanitizes LLM-generated patch inputs:
/// - Strips markdown code blocks (```diff ... ```)
/// - Strips prose preambles before the diff
/// - Adds missing leading space to context lines
/// - Recomputes exact hunk line counts in @@ headers so diffy never rejects count mismatches
fn sanitize_and_repair_patch(raw: &str) -> Result<(String, Vec<ParsedHunk>), String> {
    let mut s = raw.trim_matches(|c| c == '\n' || c == '\r');
    if s.trim_start().starts_with("```") {
        s = s.trim_start();
        if let Some(pos) = s.find('\n') {
            s = &s[pos + 1..];
        }
        let trimmed_tail = s.trim_end();
        if let Some(stripped) = trimmed_tail.strip_suffix("```") {
            s = stripped;
        }
    }
    let s = s.trim_matches(|c| c == '\n' || c == '\r');

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

    for (rel_idx, line) in raw_lines[start_idx..].iter().enumerate() {
        let line_no_cr = line.trim_end_matches('\r');
        let trimmed_start = line_no_cr.trim_start();

        // Inside a hunk `--- x` is a removed `-- x` line (an SQL or Lua comment): it only starts
        // another file when a `+++ ` header follows it.
        let is_file_header = if current_hunk.is_none() {
            trimmed_start.starts_with("--- ") || trimmed_start.starts_with("+++ ") || trimmed_start.starts_with("diff --git ")
        } else {
            let next_is_plus_header = raw_lines
                .get(start_idx + rel_idx + 1)
                .is_some_and(|next| next.starts_with("+++ "));
            line_no_cr.starts_with("diff --git ") || (line_no_cr.starts_with("--- ") && next_is_plus_header)
        };

        if is_file_header {
            if current_hunk.is_some() || !hunks.is_empty() {
                return Err(
                    "Multi-file patches are not supported in a single apply_patch call; please patch one file at a time.".to_string(),
                );
            }
            if trimmed_start.starts_with("--- ") || trimmed_start.starts_with("+++ ") {
                header_lines.push(trimmed_start.trim_end().to_string());
            }
            continue;
        }

        // Only treat `@@` as a hunk header if it starts at column 0 (or before any hunk has started)
        let is_hunk_header = if current_hunk.is_none() {
            trimmed_start.starts_with("@@")
        } else {
            line_no_cr.starts_with("@@")
        };

        if is_hunk_header {
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
            let has_remaining_diff_lines = raw_lines[start_idx + rel_idx + 1..]
                .iter()
                .any(|l| l.starts_with('+') || l.starts_with('-') || l.starts_with("@@"));
            if trimmed_start.trim_end() == "```" && !has_remaining_diff_lines {
                break;
            }
            if line_no_cr.is_empty() {
                hunk.lines.push((' ', String::new()));
                continue;
            }
            let first_char = line_no_cr.chars().next().unwrap_or(' ');
            match first_char {
                '+' | '-' | ' ' => {
                    hunk.lines.push((first_char, line_no_cr[1..].to_string()));
                }
                '\\' if line_no_cr.starts_with("\\ No newline") => {
                    // Ignore "\ No newline at end of file"
                }
                _ => {
                    // Model forgot leading space for context line
                    hunk.lines.push((' ', line_no_cr.to_string()));
                }
            }
        }
    }

    if let Some(h) = current_hunk {
        hunks.push(h);
    }

    if has_unsupported_hunk && hunks.is_empty() {
        // Return raw patch untouched so diffy's no-op behavior applies and rejection triggers
        return Ok((raw.to_string(), Vec::new()));
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

    Ok((repaired_diff, hunks))
}


/// The operation and path of a `*** Begin Patch` envelope (the format Codex and many models
/// write), from its first `*** Update/Add/Delete File:` line.
fn codex_target(patch: &str) -> Option<(&'static str, String)> {
    patch.lines().find_map(|l| {
        let l = l.trim();
        [("update", "*** Update File:"), ("add", "*** Add File:"), ("delete", "*** Delete File:")]
            .iter()
            .find_map(|(op, prefix)| l.strip_prefix(prefix).map(|p| (*op, p.trim().to_string())))
    })
}

/// Rewrites a `*** Begin Patch` envelope as a unified diff for one file. Its hunks carry no line
/// numbers, so each is located in `original` (the old side, or the new side when the patch was
/// already applied) to give the rest of the pipeline real positions.
fn codex_to_unified(patch: &str, original: Option<&str>) -> Result<String, String> {
    let (op, path) = codex_target(patch).ok_or("no '*** Update File:' line")?;
    let files = patch.lines().filter(|l| {
        let l = l.trim_start();
        l.starts_with("*** Update File:") || l.starts_with("*** Add File:") || l.starts_with("*** Delete File:")
    });
    if files.count() > 1 {
        return Err("Multi-file patches are not supported in a single apply_patch call; please patch one file at a time.".into());
    }
    if patch.lines().any(|l| l.trim_start().starts_with("*** Move to:")) {
        return Err("'*** Move to:' is not supported: move the file with a shell command, then patch it.".into());
    }
    let original = original.unwrap_or("").replace("\r\n", "\n");
    let file: Vec<&str> = original.lines().collect();

    if op == "delete" {
        if file.is_empty() {
            return Err(format!("Cannot delete {path}: it does not exist or is empty."));
        }
        let body: String = file.iter().map(|l| format!("-{l}\n")).collect();
        return Ok(format!("--- a/{path}\n+++ /dev/null\n@@ -1,{} +0,0 @@\n{body}", file.len()));
    }

    let body = patch
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("*** Update File:") && !l.trim_start().starts_with("*** Add File:"))
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with("*** End Patch"))
        .filter(|l| l.trim() != "*** End of File");
    if op == "add" {
        let lines: Vec<String> = body.map(|l| l.strip_prefix('+').unwrap_or(l).to_string()).collect();
        let text: String = lines.iter().map(|l| format!("+{l}\n")).collect();
        return Ok(format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n{text}", lines.len()));
    }

    let mut hunks: Vec<Vec<(char, String)>> = Vec::new();
    for line in body {
        if line.starts_with("@@") {
            hunks.push(Vec::new());
            continue;
        }
        if hunks.is_empty() {
            hunks.push(Vec::new());
        }
        let (tag, text) = match line.chars().next() {
            Some(c @ ('+' | '-' | ' ')) => (c, &line[1..]),
            // Models often drop the leading space of a blank context line.
            _ => (' ', line),
        };
        hunks.last_mut().unwrap().push((tag, text.to_string()));
    }
    hunks.retain(|h| h.iter().any(|(c, _)| *c != ' '));
    if hunks.is_empty() {
        return Err("the patch has no + or - lines".into());
    }

    let find = |needle: &[&str], from: usize| -> Option<usize> {
        if needle.is_empty() {
            return None;
        }
        (from..=file.len().saturating_sub(needle.len())).find(|&i| file[i..i + needle.len()] == *needle)
    };
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let (mut cursor, mut shift) = (0usize, 0isize);
    for hunk in &hunks {
        let old: Vec<&str> = hunk.iter().filter(|(c, _)| *c != '+').map(|(_, l)| l.as_str()).collect();
        let new: Vec<&str> = hunk.iter().filter(|(c, _)| *c != '-').map(|(_, l)| l.as_str()).collect();
        let (at, matched) = match find(&old, cursor) {
            Some(i) => (i, old.len()),
            None => find(&new, cursor).map_or((cursor, 0), |i| (i, new.len())),
        };
        cursor = at + matched;
        let old_start = at + 1;
        let new_start = (old_start as isize + shift).max(1);
        shift += new.len() as isize - old.len() as isize;
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", old_start, old.len(), new_start, new.len()));
        for (tag, text) in hunk {
            out.push_str(&format!("{tag}{text}\n"));
        }
    }
    Ok(out)
}

/// True when every hunk that adds lines finds its post-image already in place (within a few
/// lines of where it would go): the patch was applied before, and applying it again would only
/// duplicate the added lines.
fn already_applied(original: &str, hunks: &[ParsedHunk]) -> bool {
    let text = original.replace("\r\n", "\n");
    let file: Vec<&str> = text.lines().collect();
    let adding: Vec<&ParsedHunk> = hunks.iter().filter(|h| h.lines.iter().any(|(c, _)| *c == '+')).collect();
    !adding.is_empty()
        && adding.iter().all(|h| {
            let post: Vec<&str> = h.lines.iter().filter(|(c, _)| *c != '-').map(|(_, l)| l.as_str()).collect();
            let at = h.new_start.saturating_sub(1);
            (at.saturating_sub(3)..=at + 3).any(|start| file.get(start..start + post.len()) == Some(&post[..]))
        })
}

fn apply_fuzzy_hunks(original: &str, hunks: &[ParsedHunk]) -> Result<String, String> {
    if hunks.is_empty() {
        return Err("No hunks found in patch".to_string());
    }

    let (source, used_crlf) = ApplyPatchTool::normalize_line_endings(original);
    let lacks_final_newline = !source.is_empty() && !source.ends_with('\n');

    let mut file_lines: Vec<String> = source.lines().map(|s| s.to_string()).collect();
    let mut any_applied = false;
    let mut line_offset: isize = 0;

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        let old_lines: Vec<&str> = hunk
            .lines
            .iter()
            .filter(|(c, _)| *c == ' ' || *c == '-')
            .map(|(_, l)| l.as_str())
            .collect();

        if old_lines.is_empty() {
            let new_lines: Vec<String> = hunk
                .lines
                .iter()
                .filter(|(c, _)| *c == ' ' || *c == '+')
                .map(|(_, l)| l.clone())
                .collect();
            let added = new_lines.len() as isize;
            let pos = ((hunk.new_start as isize + line_offset).max(1) as usize)
                .saturating_sub(1)
                .min(file_lines.len());
            for (idx, nl) in new_lines.into_iter().enumerate() {
                file_lines.insert(pos + idx, nl);
            }
            line_offset += added;
            any_applied = true;
            continue;
        }

        let hinted_start = (hunk.old_start as isize + line_offset).max(1) as usize;
        let match_range = find_hunk_match(&file_lines, &old_lines, hinted_start);
        if let Some((start_idx, end_idx, inner_only)) = match_range {
            // Build replacement preserving the original file's exact context lines (`c == ' '`)
            // so a patch with slightly drifted indentation on context lines never corrupts untouched lines.
            let mut replacement = Vec::new();
            let mut old_cursor = start_idx;
            for &(op, ref text) in &hunk.lines {
                match op {
                    ' ' => {
                        if let Some(orig_line) = file_lines.get(old_cursor) {
                            replacement.push(orig_line.clone());
                        } else {
                            replacement.push(text.clone());
                        }
                        old_cursor += 1;
                    }
                    '-' => {
                        old_cursor += 1;
                    }
                    '+' => {
                        replacement.push(text.clone());
                    }
                    _ => {}
                }
            }

            let first_is_ctx = hunk.lines.first().map(|(c, _)| *c == ' ').unwrap_or(false);
            let last_is_ctx = hunk.lines.last().map(|(c, _)| *c == ' ').unwrap_or(false);
            let old_span = end_idx.saturating_sub(start_idx) as isize;
            let new_span = replacement.len() as isize;
            if inner_only && first_is_ctx && last_is_ctx && replacement.len() >= 2 && end_idx >= start_idx + 2 {
                let inner_new = replacement[1..replacement.len() - 1].to_vec();
                file_lines.splice(start_idx + 1..end_idx - 1, inner_new);
            } else {
                file_lines.splice(start_idx..end_idx, replacement);
            }
            line_offset += new_span - old_span;
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

fn find_hunk_match(file_lines: &[String], old_lines: &[&str], hint_line: usize) -> Option<(usize, usize, bool)> {
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
            return Some((i, i + target_len, false));
        }
    }

    // Pass 2: Ignore trailing whitespace
    for &i in &candidates {
        if file_lines[i..i + target_len]
            .iter()
            .zip(old_lines.iter())
            .all(|(f, o)| f.trim_end() == o.trim_end())
        {
            return Some((i, i + target_len, false));
        }
    }

    // Pass 3: Ignore indentation / full whitespace trim
    for &i in &candidates {
        if file_lines[i..i + target_len]
            .iter()
            .zip(old_lines.iter())
            .all(|(f, o)| f.trim() == o.trim())
        {
            return Some((i, i + target_len, false));
        }
    }

    // Pass 4: Fuzz 1 - ignore first and last context line if hunk has >= 3 lines and is within 100 lines of hint
    if target_len >= 3 {
        let sub_old = &old_lines[1..target_len - 1];
        let sub_len = sub_old.len();
        let non_trivial = sub_old.iter().any(|l| l.trim().len() > 2);
        if non_trivial {
            for &i in &candidates {
                if i.abs_diff(hint_idx) <= 100
                    && i + 1 + sub_len <= file_lines.len()
                    && file_lines[i + 1..i + 1 + sub_len]
                        .iter()
                        .zip(sub_old.iter())
                        .all(|(f, o)| f.trim() == o.trim())
                {
                    return Some((i, i + target_len, true));
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

    fn needs_confirmation(&self, args: &serde_json::Value, context: &ToolContext) -> bool {
        let patch_opt = args.get("patch")
            .or_else(|| args.get("input"))
            .or_else(|| args.get("diff"))
            .and_then(|v| v.as_str())
            .or_else(|| args.as_str());
        let file_opt = patch_opt
            .and_then(Self::extract_file_path)
            .or_else(|| {
                args.get("file_path")
                    .or_else(|| args.get("path"))
                    .or_else(|| args.get("file"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });
        if let Some(rel) = file_opt {
            let path = resolve_path(&context.workspace_dir, &rel);
            if !crate::fs_tools::is_within_workspace(&path, &context.workspace_dir)
                || crate::fs_tools::is_sensitive_system_path(&path)
                || crate::fs_tools::is_protected_write_path(&path)
            {
                return true;
            }
        }
        !crate::fs_tools::is_file_edit_allowed(&context.allowed_commands)
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
        let converted;
        let patch_str = if let Some((_, path)) = codex_target(patch_str) {
            let original = fs::read_to_string(resolve_path(&context.workspace_dir, &path)).ok();
            match codex_to_unified(patch_str, original.as_deref()) {
                Ok(unified) => {
                    converted = unified;
                    converted.as_str()
                }
                Err(e) => return Ok(ToolOutput::error(format!("Error: {e}"))),
            }
        } else {
            patch_str
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
        if crate::fs_tools::is_sensitive_system_path(&target_path) {
            return Ok(ToolOutput::error(format!(
                "Access denied: patching sensitive credential or authentication file '{}' is forbidden.",
                target_file
            )));
        }
        if crate::fs_tools::is_protected_write_path(&target_path) {
            return Ok(ToolOutput::error(format!(
                "Refusing to patch protected path '{}': shell startup files, git hooks and cron \
                 entries can execute arbitrary code. Edit it manually if this is intentional.",
                target_file
            )));
        }

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

        // Models wrap patches in fences and miscount hunks: clean that up before parsing.
        let (repaired_patch, hunks) = match sanitize_and_repair_patch(patch_str) {
            Ok(res) => res,
            Err(err) => return Ok(ToolOutput::error(err)),
        };
        let normalized_patch = Self::normalize_patch(&repaired_patch);

        if let Err(msg) = check_observed(&target_path, &target_file) {
            return Ok(ToolOutput::error(msg));
        }

        if already_applied(&original_content, &hunks) {
            return Ok(ToolOutput::error(format!(
                "The patch is already applied to {}: its added lines are already in place. Read the file to confirm instead of applying it again.",
                target_file
            )));
        }

        let mut patch_result = None;
        if let Ok(diffy_patch) = Patch::from_str(normalized_patch.as_ref()) {
            if let Ok(res) = Self::apply_tolerating_format_differences(&original_content, &diffy_patch) {
                if res != original_content {
                    patch_result = Some(res);
                }
            }
        }

        // Strict application failed or changed nothing: retry matching each hunk by content.
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

        if Self::deletes_target(patch_str) {
            if !patched_content.is_empty() {
                return Ok(ToolOutput::error(format!(
                    "The patch deletes {} (+++ /dev/null) but does not remove all of its lines. Read the file again and retry.",
                    target_file
                )));
            }
            if let Err(e) = fs::remove_file(&target_path) {
                return Ok(ToolOutput::error(format!("Failed to delete {}: {}", target_path.display(), e)));
            }
            return Ok(ToolOutput::success_with_summary(
                format!("Deleted {} successfully via apply_patch.", target_file),
                format!("Deleted {}", target_file),
            ));
        }

        if let Err(e) = atomic_write(&target_path, &patched_content) {
            return Ok(ToolOutput::error(format!("Failed to write patched content to {}: {}", target_path.display(), e)));
        }
        observe_file(&target_path);

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
    #[tokio::test]
    async fn codex_begin_patch_envelopes_apply() {
        let ws = std::env::temp_dir().join(format!("corex_patch_codex_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let calc = ws.join("calc.py");
        fs::write(&calc, "def add(a, b):\n    return a - b\n\n\ndef mean(values):\n    return sum(values) / len(values)\n").unwrap();
        observe_file(&calc);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        let run = |patch: String| {
            let context = &context;
            async move { ApplyPatchTool.execute(json!({ "patch": patch }), context).await.unwrap() }
        };
        // What DeepSeek actually sent: absolute path, two hunks, a blank context line without its space.
        let update = format!(
            "*** Begin Patch\n*** Update File: {}\n@@ def add(a, b):\n-    return a - b\n+    return a + b\n\n@@\n def mean(values):\n-    return sum(values) / len(values)\n+    if not values:\n+        return 0\n+    return sum(values) / len(values)\n*** End Patch\n",
            calc.display()
        );
        let out = run(update.clone()).await;
        assert!(!out.is_error, "{}", out.output);
        let text = fs::read_to_string(&calc).unwrap();
        assert!(text.contains("return a + b") && text.contains("if not values:"), "{text}");
        assert!(run(update).await.output.contains("already applied"));

        let add = "*** Begin Patch\n*** Add File: notes.txt\n+uno\n+dos\n*** End Patch\n".to_string();
        assert!(!run(add).await.is_error);
        assert_eq!(fs::read_to_string(ws.join("notes.txt")).unwrap(), "uno\ndos\n");

        observe_file(&ws.join("notes.txt"));
        let delete = "*** Begin Patch\n*** Delete File: notes.txt\n*** End Patch\n".to_string();
        assert!(!run(delete).await.is_error);
        assert!(!ws.join("notes.txt").exists());

        let two = "*** Begin Patch\n*** Update File: a.py\n@@\n-x\n+y\n*** Update File: b.py\n@@\n-x\n+y\n*** End Patch\n".to_string();
        assert!(run(two).await.output.contains("one file at a time"));
        let moved = "*** Begin Patch\n*** Update File: calc.py\n*** Move to: other.py\n@@\n-x\n+y\n*** End Patch\n".to_string();
        assert!(run(moved).await.is_error);
        let _ = fs::remove_dir_all(&ws);
        assert_eq!(ApplyPatchTool::extract_file_path("*** Begin Patch\n*** Update File: src/a.rs\n").as_deref(), Some("src/a.rs"));
    }

    #[tokio::test]
    async fn applying_the_same_patch_twice_does_not_duplicate_lines() {
        let ws = std::env::temp_dir().join(format!("corex_patch_twice_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let target = ws.join("lib.rs");
        // The same block sits further down, away from where the patch goes: that must not count.
        let far: String = (0..40).map(|i| format!("// filler {i}\n")).collect();
        fs::write(&target, format!("fn a() {{\n}}\n{far}fn a() {{\n    x();\n}}\n")).unwrap();
        observe_file(&target);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        let patch = "--- a/lib.rs\n+++ b/lib.rs\n@@ -1,2 +1,3 @@\n fn a() {\n+    x();\n }\n";

        let first = ApplyPatchTool.execute(json!({"patch": patch}), &context).await.unwrap();
        assert!(!first.is_error, "{}", first.output);
        let second = ApplyPatchTool.execute(json!({"patch": patch}), &context).await.unwrap();
        let content = fs::read_to_string(&target).unwrap();
        let _ = fs::remove_dir_all(&ws);
        assert!(second.is_error && second.output.contains("already applied"), "{}", second.output);
        assert_eq!(content.matches("    x();").count(), 2, "one from the patch, one that was already further down");
        assert!(content.starts_with("fn a() {\n    x();\n}\n// filler 0"));
    }

    #[tokio::test]
    async fn a_dev_null_target_deletes_the_file() {
        let ws = std::env::temp_dir().join(format!("corex_patch_delete_{}", std::process::id()));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        let target = ws.join("gone.txt");
        fs::write(&target, "uno\ndos\n").unwrap();
        observe_file(&target);
        let context = ToolContext {
            workspace_dir: ws.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };

        let partial = "--- a/gone.txt\n+++ /dev/null\n@@ -1,2 +0,1 @@\n-uno\n dos\n";
        let out = ApplyPatchTool.execute(json!({"patch": partial}), &context).await.unwrap();
        assert!(out.is_error && target.exists(), "a deletion that leaves lines must not pass");

        let delete = "--- a/gone.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-uno\n-dos\n";
        let out = ApplyPatchTool.execute(json!({"patch": delete}), &context).await.unwrap();
        let _ = fs::remove_dir_all(&ws);
        assert!(!out.is_error, "{}", out.output);
        assert!(out.output.starts_with("Deleted"));
        assert!(!target.exists());
    }

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
        fs::write(&target, "uno\ndos\ntres").unwrap();
        crate::fs_tools::observe_file(&target); // deliberately unterminated
        crate::fs_tools::observe_file(&target);

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
            extra_dirs: Vec::new(),
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
        crate::fs_tools::observe_file(&target);

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
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
        crate::fs_tools::observe_file(&target);

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
            extra_dirs: Vec::new(),
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
        crate::fs_tools::observe_file(&target);

        // Patch was written without trailing whitespace
        let patch = "--- a/code.py\n+++ b/code.py\n@@ -1,3 +1,3 @@\n def hello():\n-    x = 10\n+    x = 20\n return x\n";

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
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

    #[test]
    fn test_extract_file_path_handles_double_slash_and_git_headers() {
        let diff_with_double_slash = "--- a//home/sluisr/Juegos/Projects/cx/crates/corex-tools/src/subagent.rs\n+++ b//home/sluisr/Juegos/Projects/cx/crates/corex-tools/src/subagent.rs\n@@ -1,1 +1,1 @@";
        let path = ApplyPatchTool::extract_file_path(diff_with_double_slash);
        assert_eq!(path.as_deref(), Some("/home/sluisr/Juegos/Projects/cx/crates/corex-tools/src/subagent.rs"));

        let diff_git = "diff --git a/foo.rs b/foo.rs\n--- /dev/null\n+++ b/foo.rs\n";
        let path = ApplyPatchTool::extract_file_path(diff_git);
        assert_eq!(path.as_deref(), Some("foo.rs"));
    }

    #[test]
    fn test_extract_file_path_handles_spaces_in_path() {
        let diff_with_spaces = "--- a/path with spaces/file.rs\n+++ b/path with spaces/file.rs\t2026-01-01 00:00:00\n@@ -1,1 +1,1 @@";
        let path = ApplyPatchTool::extract_file_path(diff_with_spaces);
        assert_eq!(path.as_deref(), Some("path with spaces/file.rs"));

        let diff_git_spaces = "diff --git a/path with spaces/file.rs b/path with spaces/file.rs\n@@ -1,1 +1,1 @@";
        let path = ApplyPatchTool::extract_file_path(diff_git_spaces);
        assert_eq!(path.as_deref(), Some("path with spaces/file.rs"));
    }

    #[tokio::test]
    async fn test_execute_rejects_multi_file_patches() {
        let dir = unique_temp_dir("multi_file");
        let target = dir.join("first.txt");
        fs::write(&target, "alpha\nbeta\n").unwrap();

        let multi_patch = "--- a/first.txt\n+++ b/first.txt\n@@ -1,2 +1,2 @@\n alpha\n-beta\n+BETA\n--- a/second.txt\n+++ b/second.txt\n@@ -1,1 +1,1 @@\n-foo\n+bar\n";

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };

        let out = ApplyPatchTool
            .execute(json!({ "patch": multi_patch, "file_path": "first.txt" }), &context)
            .await
            .expect("execute must return ToolOutput");

        assert!(out.is_error, "multi-file patch must be rejected");
        assert!(out.output.contains("Multi-file patches are not supported"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "alpha\nbeta\n");

        fs::remove_dir_all(&dir).unwrap();
    }

    async fn patch_file(name: &str, original: &str, patch: &str) -> (bool, String) {
        let dir = unique_temp_dir(name);
        let target = dir.join("f.txt");
        fs::write(&target, original).unwrap();
        crate::fs_tools::observe_file(&target);
        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };
        let out = ApplyPatchTool
            .execute(json!({ "patch": patch, "file_path": "f.txt" }), &context)
            .await
            .expect("execute must return ToolOutput");
        let result = fs::read_to_string(&target).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        (out.is_error, result)
    }

    #[tokio::test]
    async fn removing_a_double_dash_comment_is_not_a_second_file() {
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,2 @@\n SELECT 1;\n--- old comment\n SELECT 2;\n";
        let (is_error, result) = patch_file("sql_comment", "SELECT 1;\n-- old comment\nSELECT 2;\n", patch).await;
        assert!(!is_error);
        assert_eq!(result, "SELECT 1;\nSELECT 2;\n");
    }

    #[tokio::test]
    async fn crlf_file_without_final_newline_keeps_no_stray_cr() {
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n alpha\n-beta\n+BETA\n";
        let (is_error, result) = patch_file("crlf_eof", "alpha\r\nbeta", patch).await;
        assert!(!is_error);
        assert_eq!(result, "alpha\r\nBETA");
    }

    #[tokio::test]
    async fn test_execute_preserves_trailing_spaces_in_patch_lines() {
        let dir = unique_temp_dir("trailing_spaces");
        let target = dir.join("trail.txt");
        fs::write(&target, "ctx  \nold\n").unwrap();
        crate::fs_tools::observe_file(&target);

        let patch = "--- a/trail.txt\n+++ b/trail.txt\n@@ -1,2 +1,2 @@\n ctx  \n-old\n+new   \n";

        let context = ToolContext {
            workspace_dir: dir.clone(),
            yolo_mode: true,
            sudo_password: None,
            allowed_commands: Vec::new(),
            extra_dirs: Vec::new(),
        };

        let out = ApplyPatchTool
            .execute(json!({ "patch": patch, "file_path": "trail.txt" }), &context)
            .await
            .expect("execute must succeed");

        assert!(!out.is_error, "patch should apply cleanly: {}", out.output);
        assert_eq!(fs::read_to_string(&target).unwrap(), "ctx  \nnew   \n");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_apply_patch_needs_confirmation_respects_session_allowed_commands() {
        let patch = "--- a/test.rs\n+++ b/test.rs\n@@ -1,1 +1,1 @@";
        let mut context = ToolContext {
            workspace_dir: std::path::PathBuf::from("/tmp"),
            yolo_mode: false,
            sudo_password: None,
            allowed_commands: vec!["edit".to_string()],
            extra_dirs: Vec::new(),
        };

        // When "edit" is allowed, apply_patch must NOT require confirmation
        assert!(!ApplyPatchTool.needs_confirmation(&json!({ "patch": patch }), &context));

        // When allowed_commands is empty, it MUST require confirmation
        context.allowed_commands.clear();
        assert!(ApplyPatchTool.needs_confirmation(&json!({ "patch": patch }), &context));
    }
}



