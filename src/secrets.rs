//! 密钥存储（决策 33 隐私要求，2026-10-26）
//!
//! 关键原则：
//! 1. **不读取其它应用的凭据文件**（`opencode.json`、`.credentials.yaml` 等），密钥只由用户手输
//! 2. **落盘即密文**：Windows 下用 DPAPI（`CryptProtectData`，绑定当前用户账户），换机/换用户无法解密
//! 3. **不落日志**：调用方解密到内存使用，日志只记录 provider id
//!
//! 文件格式：`{"providers": {"<id>": "v2:<hex(DPAPI 密文)>"}}`；
//! 历史明文 Key（无 `v2:` 前缀）仍可读取，调用 `migrate` 会就地升级为密文。

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 密文前缀，用于区分历史明文
const CIPHER_PREFIX: &str = "v2:";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretsFile {
    #[serde(default)]
    pub providers: BTreeMap<String, String>,
}

pub fn secrets_path(data_root: &Path) -> PathBuf {
    data_root.join("secrets.json")
}

pub fn load(data_root: &Path) -> SecretsFile {
    let path = secrets_path(data_root);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| match serde_json::from_str::<SecretsFile>(&text) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "secrets parse failed");
                None
            }
        })
        .unwrap_or_default()
}

/// 读取并解密。历史明文会原样返回（随后由 `migrate` 升级加密）。
pub fn get(data_root: &Path, provider_id: &str) -> Option<String> {
    let raw = load(data_root).providers.get(provider_id)?.clone();
    if raw.trim().is_empty() {
        return None;
    }
    match decode_value(&raw) {
        Some(plain) if !plain.trim().is_empty() => Some(plain),
        Some(_) => None,
        None => {
            tracing::warn!(provider = provider_id, "密钥解密失败（可能换用户或换机器）");
            None
        }
    }
}

pub fn has(data_root: &Path, provider_id: &str) -> bool {
    get(data_root, provider_id).is_some()
}

pub fn set(data_root: &Path, provider_id: &str, api_key: Option<&str>) -> Result<()> {
    let mut file = load(data_root);
    match api_key.map(str::trim) {
        Some(value) if !value.is_empty() => {
            let cipher = encode_value(value).ok_or_else(|| {
                crate::error::AppError::Config("密钥加密失败，已放弃保存".to_string())
            })?;
            file.providers.insert(provider_id.to_string(), cipher);
        }
        Some(_) => {
            file.providers.remove(provider_id);
        }
        None => {}
    }
    save(data_root, &file)
}

pub fn remove(data_root: &Path, provider_id: &str) -> Result<()> {
    let mut file = load(data_root);
    file.providers.remove(provider_id);
    save(data_root, &file)
}

/// 把历史明文密钥就地升级为密文。返回升级条数。
pub fn migrate(data_root: &Path) -> Result<usize> {
    let mut file = load(data_root);
    let mut upgraded = 0;
    for value in file.providers.values_mut() {
        if value.starts_with(CIPHER_PREFIX) || value.trim().is_empty() {
            continue;
        }
        if let Some(cipher) = encode_value(value) {
            *value = cipher;
            upgraded += 1;
        }
    }
    if upgraded > 0 {
        save(data_root, &file)?;
        tracing::info!(count = upgraded, "已把明文密钥升级为 DPAPI 密文");
    }
    Ok(upgraded)
}

fn encode_value(plain: &str) -> Option<String> {
    let cipher = dpapi::protect(plain.as_bytes())?;
    Some(format!("{CIPHER_PREFIX}{}", hex_encode(&cipher)))
}

fn decode_value(raw: &str) -> Option<String> {
    let Some(hex_text) = raw.strip_prefix(CIPHER_PREFIX) else {
        return Some(raw.to_string()); // 历史明文
    };
    let cipher = hex_decode(hex_text)?;
    let plain = dpapi::unprotect(&cipher)?;
    String::from_utf8(plain).ok()
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}

fn save(data_root: &Path, file: &SecretsFile) -> Result<()> {
    let path = secrets_path(data_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(file)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(&temp, &path)?;
    Ok(())
}

#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;
    use std::ptr;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        if plain.is_empty() {
            return None;
        }
        let input = DataBlob {
            cb_data: plain.len() as u32,
            pb_data: plain.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 || output.pb_data.is_null() {
            return None;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
        unsafe {
            LocalFree(output.pb_data as *mut c_void);
        }
        Some(bytes)
    }

    pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
        if cipher.is_empty() {
            return None;
        }
        let input = DataBlob {
            cb_data: cipher.len() as u32,
            pb_data: cipher.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 || output.pb_data.is_null() {
            return None;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
        unsafe {
            LocalFree(output.pb_data as *mut c_void);
        }
        Some(bytes)
    }
}

#[cfg(not(windows))]
mod dpapi {
    // 非 Windows 环境退化为不加密（本项目目标平台是 Windows）
    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        Some(plain.to_vec())
    }
    pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
        Some(cipher.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-secrets-{name}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn stores_and_removes_provider_key() {
        let root = temp_root("roundtrip");
        set(&root, "deepseek", Some("sk-test")).unwrap();
        assert!(has(&root, "deepseek"));
        assert_eq!(get(&root, "deepseek").as_deref(), Some("sk-test"));

        set(&root, "deepseek", Some("")).unwrap();
        assert!(!has(&root, "deepseek"));
    }

    #[test]
    fn keeps_key_when_none_is_passed() {
        let root = temp_root("keep");
        set(&root, "relay", Some("sk-relay")).unwrap();
        set(&root, "relay", None).unwrap();
        assert_eq!(get(&root, "relay").as_deref(), Some("sk-relay"));
    }

    #[test]
    fn ciphertext_on_disk_does_not_contain_plaintext() {
        let root = temp_root("atrest");
        set(&root, "zen", Some("sk-plain-secret-123")).unwrap();
        let text = std::fs::read_to_string(secrets_path(&root)).expect("read secrets");
        assert!(!text.contains("sk-plain-secret-123"), "落盘不得出现明文");
        assert!(text.contains(CIPHER_PREFIX), "应带密文前缀");
        assert_eq!(get(&root, "zen").as_deref(), Some("sk-plain-secret-123"));
    }

    #[test]
    fn migrates_legacy_plaintext_to_cipher() {
        let root = temp_root("migrate");
        let legacy = SecretsFile {
            providers: BTreeMap::from([("old".to_string(), "sk-legacy-key".to_string())]),
        };
        save(&root, &legacy).unwrap();
        assert_eq!(get(&root, "old").as_deref(), Some("sk-legacy-key"));

        let upgraded = migrate(&root).expect("migrate");
        assert_eq!(upgraded, 1);
        let text = std::fs::read_to_string(secrets_path(&root)).expect("read secrets");
        assert!(!text.contains("sk-legacy-key"));
        assert_eq!(get(&root, "old").as_deref(), Some("sk-legacy-key"));
        assert_eq!(migrate(&root).expect("second migrate"), 0);
    }

    #[test]
    fn hex_roundtrip() {
        let bytes = vec![0u8, 1, 15, 16, 255];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("abc").is_none());
    }
}
