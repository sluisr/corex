use std::sync::{Arc, Mutex};

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};

use crate::config::Config;
use crate::types::Usage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub cache_hit: f64,
    pub cache_miss: f64,
    pub output: f64,
}

impl Rates {
    pub const FREE: Rates = Rates { cache_hit: 0.0, cache_miss: 0.0, output: 0.0 };

    const fn per_million(cache_hit: f64, cache_miss: f64, output: f64) -> Rates {
        Rates { cache_hit: cache_hit / 1_000_000.0, cache_miss: cache_miss / 1_000_000.0, output: output / 1_000_000.0 }
    }

    fn doubled(self) -> Rates {
        Rates { cache_hit: self.cache_hit * 2.0, cache_miss: self.cache_miss * 2.0, output: self.output * 2.0 }
    }

    pub fn cost(&self, usage: &Usage) -> f64 {
        let miss = usage.prompt_tokens.saturating_sub(usage.prompt_cache_hit_tokens);
        usage.prompt_cache_hit_tokens as f64 * self.cache_hit
            + miss as f64 * self.cache_miss
            + usage.completion_tokens as f64 * self.output
    }

    pub fn uncached_cost(&self, usage: &Usage) -> f64 {
        usage.prompt_tokens as f64 * self.cache_miss + usage.completion_tokens as f64 * self.output
    }
}

const DEEPSEEK_FLASH_OFF_PEAK: Rates = Rates::per_million(0.003, 0.15, 0.6);
const DEEPSEEK_PRO_OFF_PEAK: Rates = Rates::per_million(0.022, 0.66, 1.98);

// Chinese public holidays are off-peak too; they are not tracked, so those days are overestimated.
pub fn is_deepseek_peak(at: DateTime<Utc>) -> bool {
    let weekday = !matches!(at.weekday(), Weekday::Sat | Weekday::Sun);
    weekday && matches!(at.hour(), 1..=3 | 6..=9)
}

pub fn deepseek_rates_at(model: &str, at: DateTime<Utc>) -> Rates {
    let off_peak = if model.contains("pro") || model.contains("reasoner") {
        DEEPSEEK_PRO_OFF_PEAK
    } else {
        DEEPSEEK_FLASH_OFF_PEAK
    };
    if is_deepseek_peak(at) {
        off_peak.doubled()
    } else {
        off_peak
    }
}

pub fn deepseek_rates(model: &str) -> Rates {
    deepseek_rates_at(model, Utc::now())
}

pub fn rates_for(cfg: &Config, model: &str) -> Option<Rates> {
    if cfg.local_llm_enabled || model.starts_with("local") {
        Some(Rates::FREE)
    } else if cfg.is_deepseek_endpoint() {
        Some(deepseek_rates(model))
    } else {
        None
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spend {
    pub usage: Usage,
    pub cost: f64,
    pub saved: f64,
    pub unpriced_tokens: u64,
    pub local_tokens: u64,
}

impl Spend {
    pub fn add(&mut self, usage: &Usage, rates: Option<Rates>) {
        self.usage.prompt_tokens += usage.prompt_tokens;
        self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
        self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
        self.usage.completion_tokens += usage.completion_tokens;
        self.usage.total_tokens += usage.total_tokens;
        match rates {
            Some(Rates::FREE) => self.local_tokens += usage.total_tokens,
            Some(rates) => {
                let cost = rates.cost(usage);
                self.cost += cost;
                self.saved += (rates.uncached_cost(usage) - cost).max(0.0);
            }
            None => self.unpriced_tokens += usage.total_tokens,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageMeter(Arc<Mutex<Spend>>);

impl UsageMeter {
    pub fn record(&self, usage: &Usage, rates: Option<Rates>) {
        if let Ok(mut spend) = self.0.lock() {
            spend.add(usage, rates);
        }
    }

    pub fn spend(&self) -> Spend {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(prompt: u64, hit: u64, out: u64) -> Usage {
        Usage {
            prompt_tokens: prompt,
            prompt_cache_hit_tokens: hit,
            prompt_cache_miss_tokens: prompt - hit,
            completion_tokens: out,
            total_tokens: prompt + out,
        }
    }

    fn at(when: &str) -> DateTime<Utc> {
        when.parse().unwrap()
    }

    fn off_peak() -> DateTime<Utc> {
        at("2026-11-07T12:00:00Z")
    }

    #[test]
    fn each_response_is_priced_at_its_own_model() {
        let mut spend = Spend::default();
        let u = usage(1_000_000, 0, 0);
        spend.add(&u, Some(deepseek_rates_at("deepseek-flash", off_peak())));
        spend.add(&u, Some(deepseek_rates_at("deepseek-v4-pro", off_peak())));
        assert!((spend.cost - (0.15 + 0.66)).abs() < 1e-9, "{}", spend.cost);
        assert_eq!(spend.usage.prompt_tokens, 2_000_000);
    }

    #[test]
    fn peak_hours_cost_double() {
        let u = usage(1_000_000, 0, 1_000_000);
        let peak = deepseek_rates_at("deepseek-flash", at("2026-11-09T07:30:00Z")).cost(&u);
        let off = deepseek_rates_at("deepseek-flash", off_peak()).cost(&u);
        assert!((peak - 1.5).abs() < 1e-9 && (off - 0.75).abs() < 1e-9, "{peak} {off}");
    }

    #[test]
    fn peak_windows_follow_the_weekday_schedule() {
        let peak = |t: &str| is_deepseek_peak(at(t));
        assert!(!peak("2026-11-09T00:59:59Z"));
        assert!(peak("2026-11-09T01:00:00Z"));
        assert!(peak("2026-11-09T03:59:59Z"));
        assert!(!peak("2026-11-09T04:00:00Z"));
        assert!(!peak("2026-11-09T05:59:59Z"));
        assert!(peak("2026-11-13T06:00:00Z"));
        assert!(peak("2026-11-13T09:59:59Z"));
        assert!(!peak("2026-11-13T10:00:00Z"));
        assert!(!peak("2026-11-07T07:00:00Z"));
        assert!(!peak("2026-11-08T02:00:00Z"));
    }

    #[test]
    fn cache_hits_are_counted_as_savings() {
        let mut spend = Spend::default();
        spend.add(&usage(25_881, 11_000, 385), Some(deepseek_rates_at("deepseek-flash", off_peak())));
        assert_eq!(format!("{:.4}", spend.cost), "0.0025");
        assert_eq!(format!("{:.4}", spend.saved), "0.0016");
    }

    #[test]
    fn local_and_unknown_providers_are_kept_apart() {
        let mut spend = Spend::default();
        spend.add(&usage(100, 0, 10), Some(Rates::FREE));
        spend.add(&usage(200, 0, 20), None);
        assert_eq!((spend.local_tokens, spend.unpriced_tokens, spend.cost), (110, 220, 0.0));
    }

    #[test]
    fn clones_share_one_meter() {
        let meter = UsageMeter::default();
        meter.clone().record(&usage(10, 0, 5), None);
        assert_eq!(meter.spend().usage.total_tokens, 15);
    }
}
