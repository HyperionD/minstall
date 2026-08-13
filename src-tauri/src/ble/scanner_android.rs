//! Android 版设备扫描（Kotlin BluetoothAdapter 发现）。
//!
//! 当前骨架：Android 扫描通过 Kotlin（JNI）实现，Rust 侧先提供空实现保证编译。
//! 后续：JNI 调 Kotlin startDiscovery → 返回设备列表。

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

/// 扫描周边设备（Android：JNI 调 Kotlin 发现）。当前骨架返回空。
pub async fn scan(_timeout_secs: u64) -> Result<Vec<DeviceInfo>, BleError> {
    Err(BleError::ScanTimeout)
}
