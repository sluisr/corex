use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use directories::BaseDirs;

pub struct HistoryStore;

impl HistoryStore {
    fn history_file_path() -> PathBuf {
        let dir = BaseDirs::new()
            .map(|d| d.home_dir().join(".corex"))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let _ = fs::create_dir_all(&dir);
        dir.join("history")
    }

    fn legacy_history_file_path() -> Option<PathBuf> {
        BaseDirs::new().map(|d| d.home_dir().join(".uti").join("history"))
    }

    /// Loads history from ~/.corex/history with fallback to ~/.uti/history (oldest to newest)
    pub fn load() -> Vec<String> {
        let mut path = Self::history_file_path();
        if !path.exists() {
            if let Some(legacy) = Self::legacy_history_file_path() {
                if legacy.exists() {
                    path = legacy;
                }
            }
        }
        if !path.exists() {
            return Vec::new();
        }

        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let reader = BufReader::new(file);
        let mut lines = Vec::new();
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                // Deduplicate consecutive entries
                if lines.last().map(|s: &String| s.as_str()) != Some(trimmed) {
                    lines.push(trimmed.to_string());
                }
            }
        }

        // Keep at most 1000 items
        if lines.len() > 1000 {
            lines = lines[lines.len() - 1000..].to_vec();
        }

        lines
    }

    /// Appends a new prompt to ~/.corex/history
    pub fn append(entry: &str) {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return;
        }

        let path = Self::history_file_path();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{}", trimmed);
        }
    }
}
