//! 设备扫描：枚举蓝牙适配器周边设备，过滤出手环相关设备。
//!
//! 正式应用走 SPP 通道（协议笔记 4.5 节），但仍需扫描获取设备 MAC 地址；
//! 本模块用 bluer（BlueZ DBus）做经典/低功耗混合发现，`filter_relevant` 为纯函数可测。

use std::time::Duration;

use bluer::{AdapterEvent, Session};
use futures::StreamExt;

use super::errors::BleError;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub address: String,
    pub rssi: i16,
}

/// 过滤手环相关设备（纯函数）。匹配关键词取自 POC scan.py（"mi"/"band"/"xiaomi"）。
pub fn filter_relevant(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    let keywords = ["mi", "band", "xiaomi"];
    devices
        .into_iter()
        .filter(|d| keywords.iter().any(|k| d.name.to_lowercase().contains(k)))
        .collect()
}

/// 扫描周边设备（超时秒数内持续发现），返回过滤后的设备列表。
pub async fn scan(timeout_secs: u64) -> Result<Vec<DeviceInfo>, BleError> {
    let session = Session::new().await.map_err(|_| BleError::Adapter)?;
    let adapter = session
        .default_adapter()
        .await
        .map_err(|_| BleError::Adapter)?;
    adapter
        .set_powered(true)
        .await
        .map_err(|_| BleError::Adapter)?;

    let mut events = adapter
        .discover_devices()
        .await
        .map_err(|_| BleError::ScanTimeout)?;

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);

    while std::time::Instant::now() < deadline {
        // 每轮最多等 1s，用于检查超时；事件到达即处理
        match tokio::time::timeout(Duration::from_secs(1), events.next()).await {
            Ok(Some(AdapterEvent::DeviceAdded(addr))) => {
                let key = addr.to_string();
                if seen.insert(key.clone()) {
                    let (name, rssi) = match adapter.device(addr) {
                        Ok(dev) => (
                            dev.name().await.ok().flatten().unwrap_or_default(),
                            dev.rssi().await.ok().flatten().unwrap_or(0),
                        ),
                        Err(_) => (String::new(), 0),
                    };
                    out.push(DeviceInfo { name, address: key, rssi });
                }
            }
            Ok(_) => {}
            Err(_) => {} // 1s 超时：继续循环直到 deadline
        }
    }
    Ok(filter_relevant(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_relevant_devices() {
        let input = vec![
            DeviceInfo { name: "Xiaomi Smart Band 10 Pro".into(), address: "2C:0D:CF:73:D9:95".into(), rssi: -50 },
            DeviceInfo { name: "iPhone".into(), address: "AA:BB:CC:DD:EE:FF".into(), rssi: -70 },
            DeviceInfo { name: "mi band 8".into(), address: "11:22:33:44:55:66".into(), rssi: -60 },
        ];
        let out = filter_relevant(input);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.address != "AA:BB:CC:DD:EE:FF"));
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(filter_relevant(vec![]).is_empty());
    }
}
