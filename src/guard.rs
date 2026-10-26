//! Tool Guard 子层（决策 32，2026-10-22）
//!
//! 确定性工具调用守卫：**纯代码、零模型**，安全底线不允许模型自检。
//! 前置校验（白名单 + 参数 schema + 循环检测）+ 后置校验（写后自检 + 结果规范化）。
//! 路径守卫 / 权限 L0-L3 / 写前备份 / 审计已由 `tools.rs::ToolExecutor` 承担，本层不重造，只补缺。

use crate::error::{AppError, Result};
use crate::tools::ToolExecution;
use serde_json::Value;

/// 工具白名单（v1：与 `tools::tool_definitions` 现有实现对齐；append/grep 为 v2 预留，先列入合法名）
const TOOL_WHITELIST: &[&str] = &["read_file", "write_file", "append_file", "grep"];

/// 前置校验：工具名白名单 + 参数 schema（类型 / 必填）。
pub fn validate_call(name: &str, args: &Value) -> Result<()> {
    if !TOOL_WHITELIST.contains(&name) {
        return Err(AppError::InvalidInput(format!("工具不在白名单：{name}")));
    }
    match name {
        "read_file" => require_field(args, "path", "string")?,
        "write_file" => {
            require_field(args, "path", "string")?;
            let has_content = args
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());
            let has_line = args.get("line").is_some();
            if !has_content && !has_line {
                return Err(AppError::InvalidInput(
                    "write_file 需要提供 content（整体覆盖）或 line + text（替换单行）".to_string(),
                ));
            }
            if args.get("text").is_some() && args.get("line").is_none() {
                return Err(AppError::InvalidInput(
                    "write_file 的 text 必须与 line 一起使用".to_string(),
                ));
            }
        }
        "append_file" => require_field(args, "path", "string")?,
        "grep" => require_field(args, "pattern", "string")?,
        _ => unreachable!("白名单已过滤"),
    }
    Ok(())
}

fn require_field(args: &Value, key: &str, kind: &str) -> Result<()> {
    let value = args.get(key);
    let valid = match kind {
        "string" => value
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty()),
        _ => value.is_some(),
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "参数错误：缺少或类型不符的 {key}（应为 {kind}）"
        )))
    }
}

/// 循环检测：同参数重复调用 ≥ 阈值 拦截（参数完全一致才算，JSON 序列化判等）。
pub struct LoopDetector {
    threshold: usize,
    history: Vec<(String, String)>,
}

impl LoopDetector {
    pub fn new(threshold: usize) -> Self {
        Self {
            threshold: threshold.max(2),
            history: Vec::new(),
        }
    }

    /// 记录一次调用；若同参数出现次数 ≥ 阈值，返回拦截错误。
    pub fn check(&mut self, name: &str, args: &Value) -> Result<()> {
        let key = (
            name.to_string(),
            serde_json::to_string(args).unwrap_or_default(),
        );
        let count = self.history.iter().filter(|item| *item == &key).count() + 1;
        if count >= self.threshold {
            return Err(AppError::InvalidInput(format!(
                "检测到循环调用（同参数第 {count} 次，阈值 {threshold}）：{name}",
                threshold = self.threshold
            )));
        }
        self.history.push(key);
        Ok(())
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }
}

/// 写后自检的最小长度阈值（配置占位，见 docs/modules/10 §十三）。
pub const POST_CHECK_MIN_LEN: usize = 8;

/// 后置校验：写后自检（读回验证完整性，专治截断）。
/// 字节级一致性已由 `tools.rs` write 后的校验承担；这里补「长度/关键标记」增强。
pub fn post_check_write(name: &str, execution: &ToolExecution) -> Result<()> {
    if name != "write_file" {
        return Ok(());
    }
    if !execution.ok {
        return Err(AppError::InvalidInput(format!(
            "写后自检失败：工具执行未成功（{}）",
            execution
                .payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("未知错误")
        )));
    }
    let chars = execution
        .payload
        .get("chars")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if chars < POST_CHECK_MIN_LEN {
        return Err(AppError::InvalidInput(format!(
            "写后自检失败：写入内容过短（{chars} 字符 < 阈值 {POST_CHECK_MIN_LEN}），疑似截断"
        )));
    }
    Ok(())
}

/// 结果规范化：统一 `{ok, data, error}` 结构，供上层（Planner）回注。
pub fn normalize_result(execution: &ToolExecution) -> Value {
    if execution.ok {
        let mut data = execution.payload.clone();
        // 去掉内部 ok 标志，避免嵌套
        if let Value::Object(map) = &mut data {
            map.remove("ok");
        }
        serde_json::json!({
            "ok": true,
            "tool": execution.name,
            "elapsed_ms": execution.elapsed_ms,
            "data": data
        })
    } else {
        serde_json::json!({
            "ok": false,
            "tool": execution.name,
            "elapsed_ms": execution.elapsed_ms,
            "error": execution
                .payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("工具执行失败")
        })
    }
}

/// 调用指纹（供 Planner 去重 / 审计）。
pub fn call_fingerprint(name: &str, args: &Value) -> String {
    format!("{name}:{}", serde_json::to_string(args).unwrap_or_default())
}

/// 参数提取：取 path（归一化为相对项目根的展示路径由调用方负责）。
pub fn path_arg(args: &Value) -> Option<String> {
    args.get("path")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_unknown_tool() {
        let err = validate_call("delete_file", &json!({"path": "a.txt"})).unwrap_err();
        assert!(err.to_string().contains("白名单"));
    }

    #[test]
    fn rejects_missing_path() {
        assert!(validate_call("read_file", &json!({})).is_err());
        assert!(validate_call("write_file", &json!({"content": "x"})).is_err());
    }

    #[test]
    fn rejects_text_without_line() {
        assert!(validate_call("write_file", &json!({"path": "a.txt", "text": "x"})).is_err());
    }

    #[test]
    fn accepts_valid_calls() {
        assert!(validate_call("read_file", &json!({"path": "src/main.rs"})).is_ok());
        assert!(validate_call(
            "write_file",
            &json!({"path": "a.txt", "content": "hello world"})
        )
        .is_ok());
        assert!(validate_call(
            "write_file",
            &json!({"path": "a.txt", "line": 2, "text": "new line"})
        )
        .is_ok());
    }

    #[test]
    fn loop_detector_blocks_repeated_identical_call() {
        let mut detector = LoopDetector::new(3);
        let args = json!({"path": "data/workspace/demo.txt"});
        assert!(detector.check("read_file", &args).is_ok());
        assert!(detector.check("read_file", &args).is_ok());
        let err = detector.check("read_file", &args).unwrap_err();
        assert!(err.to_string().contains("循环调用"));
    }

    #[test]
    fn loop_detector_ignores_different_params() {
        let mut detector = LoopDetector::new(3);
        assert!(detector
            .check("read_file", &json!({"path": "a.txt"}))
            .is_ok());
        assert!(detector
            .check("read_file", &json!({"path": "b.txt"}))
            .is_ok());
        assert!(detector
            .check("read_file", &json!({"path": "a.txt"}))
            .is_ok()); // 未达阈值
    }

    #[test]
    fn post_check_rejects_short_write() {
        let execution = ToolExecution {
            name: "write_file".to_string(),
            ok: true,
            path: Some("a.txt".to_string()),
            backup: None,
            target_path: None,
            backup_path: None,
            elapsed_ms: 1,
            payload: json!({"ok": true, "chars": 3}),
            sensitive_source: false,
        };
        assert!(post_check_write("write_file", &execution).is_err());
    }

    #[test]
    fn normalize_flattens_result() {
        let execution = ToolExecution {
            name: "read_file".to_string(),
            ok: true,
            path: Some("a.txt".to_string()),
            backup: None,
            target_path: None,
            backup_path: None,
            elapsed_ms: 5,
            payload: json!({"ok": true, "path": "a.txt", "content": "hi"}),
            sensitive_source: false,
        };
        let value = normalize_result(&execution);
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["content"], "hi");
        assert!(value["data"].get("ok").is_none());
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(
            call_fingerprint("read_file", &json!({"path": "a.txt"})),
            call_fingerprint("read_file", &json!({"path": "a.txt"}))
        );
    }

    #[test]
    fn history_len_tracks_calls() {
        let mut detector = LoopDetector::new(3);
        detector
            .check("read_file", &json!({"path": "a.txt"}))
            .unwrap();
        assert_eq!(detector.history_len(), 1);
        detector.reset();
        assert_eq!(detector.history_len(), 0);
    }
}
