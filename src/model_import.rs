//! 模型扫描与导入（2026-10-26）
//!
//! 目标：把下载到 `models/` 的 GGUF 变成可用模型，用户只填必要信息。
//! 流程：扫描 → 文件名推断（family/params/quant）→ 生成 Modelfile → `ollama create`
//!      → 校验 → 写回 `config/models.json`（installed + grade）。
//!
//! 约束：Modelfile 只写 `FROM` 与必要 `PARAMETER`（模型纯净原则，不加 SYSTEM）。

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 扫描到的候选模型
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GgufCandidate {
    /// 相对 models 目录的路径（用于生成 Modelfile 的 FROM）
    pub relative: String,
    /// 推断出的模型名
    pub suggested_name: String,
    pub family: String,
    pub params: String,
    pub quant: String,
    pub size_mb: f64,
    /// Ollama 中是否已存在同名模型
    pub imported: bool,
    /// 建议档位：light / heavy
    pub suggested_grade: String,
    /// 分片文件标记（如 00001/00002），需要先合并
    pub shard: Option<String>,
}

/// 导入请求（必填项由用户确认）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub relative: String,
    pub model_name: String,
    /// light / heavy
    pub grade: String,
    #[serde(default)]
    pub num_ctx: u32,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub model_name: String,
    pub grade: String,
    pub modelfile: String,
    pub quantization: String,
    pub size_gb: f64,
    /// 是否顺带切换了路由/协调模型（需重启生效）
    #[serde(default)]
    pub switched_selection: bool,
}

/// 从文件名推断 family / params / quant（确定性，不读 GGUF 头）
pub fn infer_from_filename(file_name: &str) -> (String, String, String) {
    let lowered = file_name.to_lowercase();
    let stem = lowered
        .trim_end_matches(".gguf")
        .trim_end_matches(".ggml")
        .to_string();
    let base = strip_shard_marker(&stem);

    // 只按 `-` 与 `_` 切分，保留 `qwen2.5` 这类 family 中的点
    let tokens: Vec<&str> = base
        .split(['-', '_'])
        .filter(|token| !token.is_empty())
        .collect();

    let params = tokens
        .iter()
        .copied()
        .find(|token| is_param_token(token))
        .unwrap_or("")
        .to_string();

    let (quant, quant_range) = detect_quant(&tokens);

    let stop = [
        "instruct", "chat", "base", "gguf", "it", "sft", "dpo", "awq", "gptq",
    ];
    let mut family = "model".to_string();
    for (index, token) in tokens.iter().enumerate() {
        if quant_range.contains(&index) {
            continue;
        }
        if stop.contains(token) || *token == params.as_str() {
            continue;
        }
        family = token.to_string();
        break;
    }

    (family, params, quant)
}

/// 去掉分片后缀 `-00001-of-00002`
fn strip_shard_marker(stem: &str) -> String {
    let Some(index) = stem.find("-of-") else {
        return stem.to_string();
    };
    let prefix = &stem[..index];
    if let Some(dash) = prefix.rfind('-') {
        let tail = &prefix[dash + 1..];
        if !tail.is_empty() && tail.chars().all(|ch| ch.is_ascii_digit()) {
            return stem[..dash].to_string();
        }
    }
    stem.to_string()
}

fn is_param_token(token: &str) -> bool {
    let Some(rest) = token.strip_suffix('b') else {
        return false;
    };
    !rest.is_empty()
        && rest
            .parse::<f64>()
            .map(|value| value > 0.0)
            .unwrap_or(false)
}

fn is_quant_head(token: &str) -> bool {
    if matches!(token, "f16" | "f32" | "bf16") {
        return true;
    }
    let mut chars = token.chars();
    matches!(chars.next(), Some('q')) && matches!(chars.next(), Some(ch) if ch.is_ascii_digit())
}

fn is_quant_part(token: &str) -> bool {
    matches!(token, "k" | "m" | "l" | "s") || token.chars().all(|ch| ch.is_ascii_digit())
}

/// 返回量化串与它在 token 序列里的区间
fn detect_quant(tokens: &[&str]) -> (String, std::ops::Range<usize>) {
    for (index, token) in tokens.iter().enumerate() {
        if !is_quant_head(token) {
            continue;
        }
        if matches!(*token, "f16" | "f32" | "bf16") {
            return (token.to_string(), index..index + 1);
        }
        let mut end = index + 1;
        while end < tokens.len() && is_quant_part(tokens[end]) {
            end += 1;
        }
        return (tokens[index..end].join("_"), index..end);
    }
    (String::new(), 0..0)
}

/// 生成合法的 Ollama 模型名：小写 + 数字 + `-`/`_`，并去掉 family 里的点
/// （与现有命名习惯一致：`qwen2.5` → `qwen25-7b`）
pub fn suggest_name(family: &str, params: &str) -> String {
    let mut name = String::new();
    for ch in family.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            name.push(ch.to_ascii_lowercase());
        }
    }
    if !name.is_empty() && !params.is_empty() {
        name.push('-');
        name.push_str(params);
    } else if name.is_empty() {
        name.push_str("model");
    }
    name
}

/// 按参数量建议档位：≤4b → light，其余 → heavy
pub fn suggest_grade(params: &str) -> &'static str {
    let digits: String = params
        .trim_end_matches('b')
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    match digits.parse::<f64>() {
        Ok(value) if value > 0.0 && value <= 4.0 => "light",
        Ok(_) => "heavy",
        Err(_) => "heavy",
    }
}

/// 扫描 models 目录（含子目录，深度 ≤5）
pub fn scan(models_dir: &Path, installed: &[String]) -> Vec<GgufCandidate> {
    let mut candidates = Vec::new();
    let normalized: Vec<String> = installed.iter().map(|item| item.to_lowercase()).collect();
    collect(models_dir, models_dir, 0, &normalized, &mut candidates);
    candidates.sort_by(|left, right| left.relative.cmp(&right.relative));
    candidates
}

fn collect(
    root: &Path,
    current: &Path,
    depth: usize,
    installed: &[String],
    out: &mut Vec<GgufCandidate>,
) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(root, &path, depth + 1, installed, out);
            continue;
        }
        let file_name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        if !file_name.to_lowercase().ends_with(".gguf") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file_name.clone());
        let (family, params, quant) = infer_from_filename(&file_name);
        let suggested_name = suggest_name(&family, &params);
        let size_mb = fs::metadata(&path)
            .map(|meta| (meta.len() as f64) / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        let imported = installed.iter().any(|name| {
            name == &suggested_name.to_lowercase()
                || name.starts_with(&format!("{}:", suggested_name.to_lowercase()))
        });
        let shard = shard_marker(&file_name);
        out.push(GgufCandidate {
            relative,
            suggested_grade: suggest_grade(&params).to_string(),
            suggested_name,
            family,
            params,
            quant,
            size_mb: (size_mb * 10.0).round() / 10.0,
            imported,
            shard,
        });
    }
}

fn shard_marker(file_name: &str) -> Option<String> {
    let lowered = file_name.to_lowercase();
    let index = lowered.find("-of-")?;
    let prefix = &lowered[..index];
    let dash = prefix.rfind('-')?;
    let current = &prefix[dash + 1..];
    let total = &lowered[index + 4..lowered.len().saturating_sub(".gguf".len())];
    Some(format!("{current}/{total}"))
}

/// 生成 Modelfile 内容（纯净：只有 FROM 与可选 num_ctx）
pub fn build_modelfile(relative: &str, num_ctx: u32) -> String {
    let mut text = format!("FROM ./{}\n", relative.trim_start_matches("./"));
    if num_ctx > 0 {
        text.push_str(&format!("PARAMETER num_ctx {num_ctx}\n"));
    }
    text
}

/// 执行 ollama create
fn run_ollama_create(model_name: &str, modelfile: &Path) -> Result<String> {
    let output = std::process::Command::new("ollama")
        .arg("create")
        .arg(model_name)
        .arg("-f")
        .arg(modelfile)
        .output()
        .map_err(|err| {
            AppError::InvalidInput(format!(
                "无法启动 ollama 命令（是否已安装并在 PATH 中？）：{err}"
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        return Err(AppError::InvalidInput(format!(
            "ollama create 失败：{}",
            detail.trim().chars().take(400).collect::<String>()
        )));
    }
    Ok(stdout.trim().to_string())
}

/// 把模型登记到 config/models.json（保留 `_comment` 等未知字段）
pub fn register_in_models_json(
    app_root: &Path,
    name: &str,
    grade: &str,
    size_gb: f64,
    quantization: &str,
) -> Result<()> {
    let path = app_root.join("config").join("models.json");
    let text = fs::read_to_string(&path)?;
    let mut value: serde_json::Value = serde_json::from_str(&text)?;

    {
        let installed = value
            .get_mut("installed")
            .and_then(|item| item.as_object_mut())
            .ok_or_else(|| AppError::Config("models.json 缺少 installed 字段".to_string()))?;
        installed.insert(
            name.to_string(),
            serde_json::json!({
                "grade": grade,
                "quantization": if quantization.is_empty() { "Q4_K_M" } else { quantization },
                "size_gb": (size_gb * 10.0).round() / 10.0
            }),
        );
    }

    // 档位空缺时才自动填，避免覆盖用户已选
    let current = value
        .get("grades")
        .and_then(|item| item.get(grade))
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let occupied = !current.trim().is_empty()
        && value
            .get("installed")
            .and_then(|item| item.as_object())
            .map(|installed| installed.contains_key(&current))
            .unwrap_or(false);
    if !occupied {
        if let Some(grades) = value
            .get_mut("grades")
            .and_then(|item| item.as_object_mut())
        {
            grades.insert(grade.to_string(), serde_json::json!(name));
        }
    }

    let pretty = serde_json::to_string_pretty(&value)?;
    fs::write(&path, format!("{pretty}\n"))?;
    Ok(())
}

/// 完整导入流程
pub fn import(app_root: &Path, models_dir: &Path, request: &ImportRequest) -> Result<ImportResult> {
    let name = request.model_name.trim().to_lowercase();
    if name.is_empty() {
        return Err(AppError::InvalidInput("模型名不能为空".to_string()));
    }
    let first_ok = name
        .chars()
        .next()
        .map(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        .unwrap_or(false);
    let rest_ok = name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '-' | '_'));
    if !first_ok || !rest_ok {
        return Err(AppError::InvalidInput(
            "模型名只能包含小写字母、数字与 . - _，且以字母或数字开头".to_string(),
        ));
    }

    let grade = match request.grade.trim() {
        "light" => "light",
        "heavy" => "heavy",
        other => {
            return Err(AppError::InvalidInput(format!(
                "档位只能是 light 或 heavy（收到：{other}）"
            )))
        }
    };

    let relative = request.relative.trim();
    if relative.contains("..") {
        return Err(AppError::InvalidInput("路径不合法".to_string()));
    }
    let file_path = models_dir.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !file_path.is_file() {
        return Err(AppError::InvalidInput(format!(
            "找不到模型文件：{}",
            file_path.display()
        )));
    }
    let file_name = file_path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    if let Some(shard) = shard_marker(&file_name) {
        return Err(AppError::InvalidInput(format!(
            "这是分片文件（{shard}），请先用 llama-gguf-split 合并成单文件再导入"
        )));
    }

    let modelfile_text = build_modelfile(relative, request.num_ctx);
    let modelfile_path = models_dir.join(format!("Modelfile-{name}"));
    fs::write(&modelfile_path, &modelfile_text)?;

    if let Err(err) = run_ollama_create(&name, &modelfile_path) {
        // 失败回滚：删掉刚生成的 Modelfile
        let _ = fs::remove_file(&modelfile_path);
        return Err(err);
    }

    let size_gb = fs::metadata(&file_path)
        .map(|meta| (meta.len() as f64) / (1024.0 * 1024.0 * 1024.0))
        .unwrap_or(0.0);
    let quantization = infer_from_filename(&file_name).2;
    register_in_models_json(app_root, &name, grade, size_gb, &quantization)?;

    tracing::info!(
        model = %name,
        grade,
        modelfile = %modelfile_path.display(),
        "model imported"
    );

    Ok(ImportResult {
        model_name: name,
        grade: grade.to_string(),
        modelfile: modelfile_text,
        quantization,
        size_gb: (size_gb * 10.0).round() / 10.0,
        switched_selection: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_family_params_quant_from_filename() {
        let (family, params, quant) = infer_from_filename("qwen2.5-7b-instruct-q4_k_m.gguf");
        assert_eq!(family, "qwen2.5");
        assert_eq!(params, "7b");
        assert_eq!(quant, "q4_k_m");
        assert_eq!(suggest_name(&family, &params), "qwen25-7b");
    }

    #[test]
    fn handles_sharded_filenames() {
        let (family, params, _) =
            infer_from_filename("qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf");
        assert_eq!(family, "qwen2.5");
        assert_eq!(params, "7b");
        let shard = shard_marker("qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf");
        assert_eq!(shard.as_deref(), Some("00001/00002"));
    }

    #[test]
    fn suggests_grade_by_param_size() {
        assert_eq!(suggest_grade("3b"), "light");
        assert_eq!(suggest_grade("0.5b"), "light");
        assert_eq!(suggest_grade("7b"), "heavy");
        assert_eq!(suggest_grade("14b"), "heavy");
        assert_eq!(suggest_grade(""), "heavy");
    }

    #[test]
    fn builds_clean_modelfile() {
        let text = build_modelfile("qwen3-8b.gguf", 8192);
        assert!(text.starts_with("FROM ./qwen3-8b.gguf"));
        assert!(text.contains("PARAMETER num_ctx 8192"));
        assert!(!text.to_uppercase().contains("SYSTEM"), "不得写 SYSTEM");
    }

    #[test]
    fn scans_gguf_files_and_infers_metadata() {
        let root = std::env::temp_dir().join(format!(
            "ai-assistant-scan-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        fs::create_dir_all(root.join("sub")).expect("mkdir");
        fs::write(root.join("qwen2.5-3b-instruct-q4_k_m.gguf"), b"fake").expect("write");
        fs::write(root.join("sub/llama-8b-q8_0.gguf"), b"fake").expect("write");
        fs::write(root.join("readme.md"), b"x").expect("write");

        let candidates = scan(&root, &["qwen25-3b".to_string()]);
        assert_eq!(candidates.len(), 2, "只应识别 gguf");
        let qwen = candidates
            .iter()
            .find(|item| item.relative.contains("qwen2.5"))
            .expect("qwen candidate");
        assert_eq!(qwen.suggested_name, "qwen25-3b");
        assert_eq!(qwen.suggested_grade, "light");
        assert!(qwen.imported, "已存在同名模型应标记 imported");
        let llama = candidates
            .iter()
            .find(|item| item.relative.contains("llama"))
            .expect("llama candidate");
        assert_eq!(llama.suggested_grade, "heavy");
        assert!(!llama.imported);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn registers_model_and_fills_empty_grade() {
        let root = std::env::temp_dir().join(format!(
            "ai-assistant-register-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        fs::create_dir_all(root.join("config")).expect("mkdir");
        fs::write(
            root.join("config/models.json"),
            r#"{"_comment":"keep me","grades":{"light":"qwen25-3b","heavy":"","cloud":null},"installed":{"qwen25-3b":{"grade":"light","quantization":"Q4_K_M","size_gb":2.5}}}"#,
        )
        .expect("write models.json");

        register_in_models_json(&root, "qwen25-14b", "heavy", 8.9, "q4_k_m").expect("register");
        let text = fs::read_to_string(root.join("config/models.json")).expect("read");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parse");
        assert_eq!(value["_comment"], "keep me", "不得丢注释");
        assert_eq!(value["installed"]["qwen25-14b"]["grade"], "heavy");
        assert_eq!(value["grades"]["heavy"], "qwen25-14b", "空缺档位应自动填");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn keeps_existing_grade_selection() {
        let root = std::env::temp_dir().join(format!(
            "ai-assistant-register-keep-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        fs::create_dir_all(root.join("config")).expect("mkdir");
        fs::write(
            root.join("config/models.json"),
            r#"{"grades":{"light":"qwen25-3b","heavy":"qwen25-7b"},"installed":{"qwen25-3b":{"grade":"light"},"qwen25-7b":{"grade":"heavy"}}}"#,
        )
        .expect("write");

        register_in_models_json(&root, "qwen25-14b", "heavy", 8.9, "q4_k_m").expect("register");
        let text = fs::read_to_string(root.join("config/models.json")).expect("read");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parse");
        assert_eq!(
            value["grades"]["heavy"], "qwen25-7b",
            "档位已被占用时不得覆盖用户选择"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_invalid_model_name() {
        let req = ImportRequest {
            relative: "x.gguf".to_string(),
            model_name: "Bad Name!".to_string(),
            grade: "light".to_string(),
            num_ctx: 0,
            display_name: String::new(),
        };
        let root = std::env::temp_dir();
        assert!(import(&root, &root, &req).is_err());
    }
}
