use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use directories::BaseDirs;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

const MAX_CACHE_ENTRIES: usize = 100;
const MAX_REASONING_CHARS: usize = 32_000;
const MAX_RAW_KEY_BYTES: usize = 256;

/// In-memory reasoning cache with deterministic FIFO eviction.
/// Uses a HashMap for O(1) lookups and a VecDeque to track insertion order
/// for proper oldest-first eviction (not random like bare HashMap iteration).
#[derive(Clone)]
pub struct ReasoningCache {
    cache: Arc<Mutex<HashMap<String, String>>>,
    order: Arc<Mutex<VecDeque<String>>>,
    write_lock: Arc<Mutex<()>>,
    cache_file: PathBuf,
}

impl ReasoningCache {
    pub fn new() -> Self {
        let base_dir = BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".corex"))
            .unwrap_or_else(|| PathBuf::from(".corex"));

        let _ = crate::secure_fs::ensure_private_dir(&base_dir);
        let cache_file = base_dir.join("reasoning_cache.json");

        let mut raw_map = HashMap::new();
        if cache_file.exists() {
            if let Ok(meta) = fs::metadata(&cache_file) {
                if meta.len() <= 5 * 1024 * 1024 {
                    if let Ok(data) = fs::read_to_string(&cache_file) {
                        if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                            raw_map = parsed;
                            debug!("[CACHE] Loaded {} entries from disk", raw_map.len());
                        }
                    }
                }
            }
        } else if let Some(dirs) = BaseDirs::new() {
            let legacy_file = dirs.home_dir().join(".deepseek").join("reasoning_cache.json");
            if legacy_file.exists() {
                if let Ok(data) = fs::read_to_string(&legacy_file) {
                    if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&data) {
                        raw_map = parsed;
                        debug!("[CACHE] Migrated {} entries from legacy DeepSeek cache", raw_map.len());
                    }
                }
            }
        }

        let mut map = HashMap::with_capacity(raw_map.len().min(MAX_CACHE_ENTRIES));
        let mut order = VecDeque::with_capacity(raw_map.len().min(MAX_CACHE_ENTRIES));
        for (k, v) in raw_map.into_iter().take(MAX_CACHE_ENTRIES) {
            let capped_v = if v.len() > MAX_REASONING_CHARS {
                crate::types::safe_truncate_str(&v, MAX_REASONING_CHARS).to_string()
            } else {
                v
            };
            order.push_back(k.clone());
            map.insert(k, capped_v);
        }

        Self {
            cache: Arc::new(Mutex::new(map)),
            order: Arc::new(Mutex::new(order)),
            write_lock: Arc::new(Mutex::new(())),
            cache_file,
        }
    }

    pub fn compute_key(text: &str, tool_calls: Option<&[crate::types::ToolCall]>) -> String {
        let clean_text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let tool_names = tool_calls.map(|calls| {
            let mut names: Vec<String> = calls
                .iter()
                .map(|c| {
                    if c.id.is_empty() {
                        format!("{}:{}", c.function.name, c.function.arguments)
                    } else {
                        format!("{}:{}:{}", c.id, c.function.name, c.function.arguments)
                    }
                })
                .collect();
            names.sort();
            names.join(";")
        });

        let raw = match tool_names {
            Some(tools) if !tools.is_empty() => format!("{}__{}", clean_text, tools),
            _ => clean_text,
        };

        if raw.len() <= MAX_RAW_KEY_BYTES {
            raw
        } else {
            let prefix = crate::types::safe_truncate_str(&raw, 96);
            let mut hasher = Sha256::new();
            hasher.update(raw.as_bytes());
            let digest = hasher.finalize();
            let mut hex_str = String::with_capacity(64);
            for b in digest {
                use std::fmt::Write;
                let _ = write!(&mut hex_str, "{:02x}", b);
            }
            format!("{}#sha256:{}", prefix, hex_str)
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

        let capped_reasoning = if reasoning.len() > MAX_REASONING_CHARS {
            crate::types::safe_truncate_str(&reasoning, MAX_REASONING_CHARS).to_string()
        } else {
            reasoning
        };

        {
            let mut guard = match self.cache.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let mut order = match self.order.lock() {
                Ok(g) => g,
                Err(_) => return,
            };

            if !guard.contains_key(&key) {
                order.push_back(key.clone());
            }
            guard.insert(key, capped_reasoning);

            while guard.len() > MAX_CACHE_ENTRIES {
                if let Some(oldest_key) = order.pop_front() {
                    guard.remove(&oldest_key);
                } else {
                    break;
                }
            }
        }

        let path = self.cache_file.clone();
        let cache_ref = Arc::clone(&self.cache);
        let write_lock = Arc::clone(&self.write_lock);
        let write_fn = move || {
            let _wguard = match write_lock.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            let bytes = {
                let guard = match cache_ref.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                serde_json::to_vec(&*guard).ok()
            };
            if let Some(bytes) = bytes {
                if let Err(e) = crate::secure_fs::write_private_atomic(&path, &bytes) {
                    warn!("[CACHE] Failed to write reasoning cache: {}", e);
                }
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(write_fn);
        } else {
            write_fn();
        }
    }
}

impl Default for ReasoningCache {
    fn default() -> Self {
        Self::new()
    }
}
