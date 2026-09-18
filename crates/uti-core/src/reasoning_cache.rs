use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use directories::BaseDirs;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct ReasoningCache {
    cache: Arc<Mutex<HashMap<String, String>>>,
    cache_file: PathBuf,
}

impl ReasoningCache {
    pub fn new() -> Self {
        let base_dir = BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".corex"))
            .unwrap_or_else(|| PathBuf::from(".corex"));

        let _ = fs::create_dir_all(&base_dir);
        let cache_file = base_dir.join("reasoning_cache.json");

        let mut map = HashMap::new();
        if cache_file.exists() {
            if let Ok(data) = fs::read_to_string(&cache_file) {
                if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                    map = parsed;
                    debug!("[CACHE] Loaded {} entries from disk", map.len());
                }
            }
        } else {
            // Check legacy UTI and DeepSeek paths for seamless migration
            if let Some(dirs) = BaseDirs::new() {
                let uti_file = dirs.home_dir().join(".uti").join("reasoning_cache.json");
                let legacy_file = dirs.home_dir().join(".deepseek").join("reasoning_cache.json");
                if uti_file.exists() {
                    if let Ok(data) = fs::read_to_string(&uti_file) {
                        if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                            map = parsed;
                            debug!("[CACHE] Migrated {} entries from UTI cache", map.len());
                        }
                    }
                } else if legacy_file.exists() {
                    if let Ok(data) = fs::read_to_string(&legacy_file) {
                        if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                            map = parsed;
                            debug!("[CACHE] Migrated {} entries from legacy DeepSeek cache", map.len());
                        }
                    }
                }
            }
        }

        Self {
            cache: Arc::new(Mutex::new(map)),
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

        guard.insert(key, reasoning);

        // Limit size to last 100 entries
        if guard.len() > 100 {
            let keys_to_remove: Vec<String> = guard.keys().take(guard.len() - 100).cloned().collect();
            for k in keys_to_remove {
                guard.remove(&k);
            }
        }

        let data_to_save = guard.clone();
        let path = self.cache_file.clone();

        tokio::spawn(async move {
            if let Ok(serialized) = serde_json::to_string_pretty(&data_to_save) {
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = tokio::fs::write(&path, serialized).await {
                    warn!("[CACHE] Failed to save reasoning cache: {}", e);
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
