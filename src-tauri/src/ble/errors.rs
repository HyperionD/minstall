//! 蓝牙层错误类型。
//!
//! 注：正式应用走 SPP 通道（协议笔记 4.5 节），但扫描/配对仍需 BLE 适配器，
//! 故错误类型保留 BleError 命名，覆盖 SPP 连接与认证/推送全流程。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BleError {
    #[error("蓝牙适配器不可用")]
    Adapter,
    #[error("扫描超时")]
    ScanTimeout,
    #[error("连接失败: {0}")]
    ConnectFailed(String),
    #[error("认证失败: {0}")]
    AuthFailed(String),
    #[error("推送失败 (chunk {chunk}): {detail}")]
    PushFailed { chunk: usize, detail: String },
    #[error("文件错误: {0}")]
    FileError(String),
}
