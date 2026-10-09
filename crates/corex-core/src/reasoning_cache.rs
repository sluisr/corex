use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use directories::BaseDirs;
use tracing::{debug, warn};

const MAX_CACHE_ENTRIES: usize = 100;

/// In-memory reasoning cache with deterministic FIFO eviction.
/// Uses a HashMap for O(1) lookups and a VecDeque to track insertion order
/// for proper oldest-first eviction (not random like bare HashMap iteration).
#[derive(Clone)]
pub struct ReasoningCache {
    cache: Arc<Mutex<HashMap<String, String>>>,
    order: Arc<Mutex<VecDeque<String>>>,
    cache_file: PathBuf,
}

impl ReasoningCache {
    pub fn new() -> Self {
        let base_dir = BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".corex"))
            .unwrap_or_else(|| PathBuf::from(".corex"));

        let _ = crate::secure_fs::ensure_private_dir(&base_dir);
        let cache_file = base_dir.join("reasoning_cache.json");

        let mut map = HashMap::new();
        if cache_file.exists() {
            if let Ok(data) = fs::read_to_string(&cache_file) {
                if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                    map = parsed;
                    debug!("[CACHE] Loaded {} entries from disk", map.len());
                }
            }
        } else if let Some(dirs) = BaseDirs::new() {
            let legacy_file = dirs.home_dir().join(".deepseek").join("reasoning_cache.json");
            if legacy_file.exists() {
                if let Ok(data) = fs::read_to_string(&legacy_file) {
                    if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                        map = parsed;
                        debug!("[CACHE] Migrated {} entries from legacy DeepSeek cache", map.len());
                    }
                }
            }
        }

        let order: VecDeque<String> = map.keys().cloned().collect();

        Self {
            cache: Arc::new(Mutex::new(map)),
            order: Arc::new(Mutex::new(order)),
            cache_file,
        }
    }

    pub fn compute_key(text: &str, tool_calls: Option<&[crate::types::ToolCall]>) -> String {
        let clean_text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let tool_names = tool_calls.map(|calls| {
            let mut names: Vec<String> = calls
                .iter()
                .map(|c| format!("{}:{}", c.function.name, c.function.arguments))
                .collect();
            names.sort();
            names.join(";")
        });

        match tool_names {
            Some(tools) if !tools.is_empty() => format!("{}__{}", clean_text, tools),
            _ => clean_text,
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let guard = self.cache.lock().ok()?;
        guard.get(key).cloned()
    }

    pub fn insert(&self, key: String, reasoning: String) {
        if key.is_empty() || reasoning.is_empty() {
            return;
        }

        let mut guard = match self.cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let mut order = match self.order.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        // If key already exists, don't add duplicate to order queue
        if !guard.contains_key(&key) {
            order.push_back(key.clone());
        }
        guard.insert(key, reasoning);

        // Evict oldest entries (FIFO) when over capacity
        while guard.len() > MAX_CACHE_ENTRIES {
            if let Some(oldest_key) = order.pop_front() {
                guard.remove(&oldest_key);
            } else {
                break;
            }
        }

        let data_to_save = guard.clone();
        let path = self.cache_file.clone();

        tokio::task::spawn_blocking(move || {
            if let Ok(serialized) = serde_json::to_string_pretty(&data_to_save) {
                if let Err(e) = crate::secure_fs::write_private_atomic(&path, serialized.as_bytes()) {
                    warn!("[CACHE] Failed to write reasoning cache: {}", e);
                }
            }
        });
    }
}

impl Default for ReasoningCache {
    fn default() -> Self {
        Self::new()
    }
}
