//! BLE/蓝牙层：设备扫描、SPP 连接管理与错误类型。
//!
//! 平台差异：
//! - Linux：走 BlueZ（bluer），见 protocol-notes.md 4.3/4.5 节
//! - Android：蓝牙层在 Kotlin（JNI 桥），Rust 侧提供 tokio 字节流实现

pub mod errors;

#[cfg(target_os = "linux")]
pub mod connection;
#[cfg(target_os = "android")]
pub mod connection_android;
#[cfg(target_os = "android")]
pub use connection_android as connection;

#[cfg(target_os = "linux")]
pub mod scanner;
#[cfg(target_os = "android")]
pub mod scanner_android;
#[cfg(target_os = "android")]
pub use scanner_android as scanner;
