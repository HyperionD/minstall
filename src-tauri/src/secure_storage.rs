use crate::protocol::encoding::parse_authkey;

/// Normalize a user-provided authkey without retaining the original value.
pub fn normalize_authkey(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let body = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    parse_authkey(trimmed).map(|_| body.to_ascii_lowercase())
}

/// Validate and normalize an authkey before it reaches a platform store.
pub fn validate_authkey(value: &str) -> Result<String, String> {
    normalize_authkey(value).ok_or_else(|| "authkey 应为 32 位 hex 字符（可带 0x 前缀）".to_owned())
}

/// Convert a platform-store result into an optional normalized authkey.
pub fn saved_value(value: &str) -> Option<String> {
    normalize_authkey(value)
}

#[cfg(target_os = "linux")]
const KEYRING_SERVICE: &str = "minstall";
#[cfg(target_os = "linux")]
const KEYRING_USER: &str = "authkey";

#[cfg(target_os = "linux")]
fn ensure_secret_service() -> Result<(), String> {
    use std::sync::OnceLock;

    static STORE: OnceLock<Result<(), String>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let store = zbus_secret_service_keyring_store::Store::new()
                .map_err(|error| format!("无法初始化 Linux Secret Service: {error}"))?;
            keyring_core::set_default_store(store);
            Ok(())
        })
        .clone()
}

#[cfg(target_os = "linux")]
fn secret_entry() -> Result<keyring_core::Entry, String> {
    ensure_secret_service()?;
    keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|error| format!("无法初始化系统安全存储: {error}"))
}

#[cfg(target_os = "linux")]
pub fn get() -> Result<Option<String>, String> {
    use keyring_core::Error;

    match secret_entry()?.get_password() {
        Ok(value) => Ok(saved_value(&value)),
        Err(Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("读取系统安全存储失败: {error}")),
    }
}

#[cfg(target_os = "linux")]
pub fn save(value: &str) -> Result<(), String> {
    let normalized = validate_authkey(value)?;
    secret_entry()?
        .set_password(&normalized)
        .map_err(|error| format!("保存 authkey 到系统安全存储失败: {error}"))
}

#[cfg(target_os = "linux")]
pub fn clear() -> Result<(), String> {
    use keyring_core::Error;

    match secret_entry()?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("清除系统安全存储中的 authkey 失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_authkey, saved_value, validate_authkey};

    #[test]
    fn accepts_valid_authkey_and_normalizes_prefix_and_case() {
        assert_eq!(
            normalize_authkey("0xABABABABABABABABABABABABABABABAB"),
            Some("abababababababababababababababab".to_owned())
        );
    }

    #[test]
    fn rejects_invalid_authkey() {
        assert!(validate_authkey("short").is_err());
        assert!(validate_authkey("zzababababababababababababababab").is_err());
    }

    #[test]
    fn empty_saved_value_means_no_authkey() {
        assert_eq!(saved_value(""), None);
        assert_eq!(saved_value("  "), None);
        assert_eq!(saved_value("not-an-authkey"), None);
        assert_eq!(
            saved_value("abababababababababababababababab"),
            Some("abababababababababababababababab".to_owned())
        );
    }
}
