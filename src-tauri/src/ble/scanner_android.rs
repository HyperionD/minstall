//! Android 版设备扫描：JNI 调 Kotlin BleScan.startDiscovery。
//!
//! Kotlin `BleScan.scan(timeoutMs)` 返回 "name|address|rssi" 字符串数组，
//! Rust 解析后过滤手环相关设备（filter_relevant 纯函数，与 Linux 版一致）。

use jni::objects::JValue;

use super::errors::BleError;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub address: String,
    pub rssi: i16,
}

/// 过滤手环相关设备（纯函数，与 Linux 版一致）。
pub fn filter_relevant(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    let keywords = ["mi", "band", "xiaomi"];
    devices
        .into_iter()
        .filter(|d| keywords.iter().any(|k| d.name.to_lowercase().contains(k)))
        .collect()
}

/// 扫描周边设备（JNI 调 Kotlin startDiscovery）。需 BLUETOOTH_SCAN 权限。
pub async fn scan(timeout_secs: u64) -> Result<Vec<DeviceInfo>, BleError> {
    let devices = tokio::task::spawn_blocking(move || jni_scan(timeout_secs))
        .await
        .map_err(|_e| BleError::ScanTimeout)??;
    Ok(filter_relevant(devices))
}

/// JNI 调 Kotlin BleScan.scan(timeoutMs) → 解析设备列表。
fn jni_scan(timeout_secs: u64) -> Result<Vec<DeviceInfo>, BleError> {
    let vm = crate::ble::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = env
        .find_class("com/minstall/app/BleScan")
        .map_err(|e| BleError::ConnectFailed(format!("查找 BleScan 失败: {e}")))?;
    let timeout_ms = (timeout_secs * 1000) as i64;
    let arr = env
        .call_static_method(
            class,
            "scan",
            "(J)[Ljava/lang/String;",
            &[JValue::Long(timeout_ms)],
        )
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 scan 失败: {e}")))?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 返回数组失败: {e}")))?;

    let mut out = Vec::new();
    let arr_ref: jni::objects::JObjectArray = jni::objects::JObject::from(arr).into();
    let len = env
        .get_array_length(&arr_ref)
        .map_err(|e| BleError::ConnectFailed(format!("数组长度失败: {e}")))?;
    for i in 0..len {
        let elem = env
            .get_object_array_element(&arr_ref, i)
            .map_err(|e| BleError::ConnectFailed(format!("数组元素失败: {e}")))?;
        let s: String = env
            .get_string(&jni::objects::JString::from(elem))
            .map(|j| j.into())
            .unwrap_or_default();
        // 格式 "name|address|rssi"
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() >= 2 {
            out.push(DeviceInfo {
                name: parts[0].to_string(),
                address: parts[1].to_string(),
                rssi: parts.get(2).and_then(|r| r.parse().ok()).unwrap_or(0),
            });
        }
    }
    Ok(out)
}
