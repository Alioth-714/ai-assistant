//! 本地用量记账：每次外部调用追加一行 JSONL，供「费用」模块聚合。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 一次外部调用的用量记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    /// Unix 毫秒时间戳
    pub ts: u64,
    pub provider_id: String,
    pub model_id: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_tokens: u64,
    /// 供应商返回的花费（美元或人民币，取决于供应商），取不到则为 0
    pub cost: f64,
}

/// 聚合结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub cost: f64,
    pub requests: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_tokens: u64,
}

fn usage_dir(data_root: &Path) -> PathBuf {
    data_root.join("usage")
}

fn month_file(data_root: &Path, ts_ms: u64) -> PathBuf {
    let secs = (ts_ms / 1000) as i64;
    let (year, month) = year_month(secs);
    usage_dir(data_root).join(format!("{year:04}-{month:02}.jsonl"))
}

/// 由 Unix 秒推出年月（不引第三方库，按天累加）
fn year_month(secs: i64) -> (i64, i64) {
    let mut days = secs.div_euclid(86_400);
    let mut year = 1970;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let len = if leap { 366 } else { 365 };
        if days >= len {
            days -= len;
            year += 1;
        } else {
            break;
        }
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for len in months {
        if days >= len {
            days -= len;
            month += 1;
        } else {
            break;
        }
    }
    (year, month as i64)
}

/// 追加一条记录（按月分片）
pub fn record(data_root: &Path, entry: &UsageEntry) -> std::io::Result<()> {
    std::fs::create_dir_all(usage_dir(data_root))?;
    let path = month_file(data_root, entry.ts);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(entry).unwrap_or_default();
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// 读取全部记录（跨月）
pub fn entries(data_root: &Path) -> Vec<UsageEntry> {
    let dir = usage_dir(data_root);
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(&dir) else {
        return out;
    };
    for item in read.flatten() {
        if item.path().extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(item.path()) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<UsageEntry>(line) {
                out.push(entry);
            }
        }
    }
    out
}

/// 按周期聚合：today / month / total
pub fn summary(data_root: &Path, period: &str, now_ms: u64) -> UsageSummary {
    summary_of(data_root, period, now_ms, None)
}

/// 按供应商过滤的聚合（provider = None 表示全部）
pub fn summary_of(
    data_root: &Path,
    period: &str,
    now_ms: u64,
    provider: Option<&str>,
) -> UsageSummary {
    let all = entries(data_root);
    let now_secs = (now_ms / 1000) as i64;
    let today_start = now_secs.div_euclid(86_400) * 86_400;
    let (year, month) = year_month(now_secs);
    let month_start = month_start_secs(year, month);

    let mut summary = UsageSummary::default();
    for entry in all {
        let secs = (entry.ts / 1000) as i64;
        let include = match period {
            "today" => secs >= today_start,
            "total" => true,
            _ => secs >= month_start,
        };
        if !include {
            continue;
        }
        if let Some(id) = provider {
            if entry.provider_id != id {
                continue;
            }
        }
        summary.cost += entry.cost;
        summary.requests += 1;
        summary.tokens_in += entry.tokens_in;
        summary.tokens_out += entry.tokens_out;
        summary.cache_tokens += entry.cache_tokens;
    }
    summary
}

fn month_start_secs(year: i64, month: i64) -> i64 {
    let mut days: i64 = 0;
    for y in 1970..year {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        days += if leap { 366 } else { 365 };
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for len in months.iter().take((month - 1) as usize) {
        days += *len as i64;
    }
    days * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_month_maps_epoch() {
        assert_eq!(year_month(0), (1970, 1));
        let (y, m) = year_month(1_700_000_000);
        assert_eq!(y, 2023);
        assert_eq!(m, 11);
    }

    #[test]
    fn records_and_aggregates() {
        let dir = std::env::temp_dir().join(format!("usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let entry = UsageEntry {
            ts: 1_700_000_000_000,
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-flash".to_string(),
            tokens_in: 100,
            tokens_out: 50,
            cache_tokens: 0,
            cost: 0.01,
        };
        record(&dir, &entry).unwrap();
        let total = summary(&dir, "total", 1_700_000_000_000);
        assert_eq!(total.requests, 1);
        assert_eq!(total.tokens_in, 100);
        assert!((total.cost - 0.01).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
