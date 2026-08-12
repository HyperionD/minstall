//! SPP 连接管理：持有 RFCOMM 流句柄，负责连接/断开。
//!
//! Band 10 Pro 的 V2 协议走经典蓝牙 SPP 通道（协议笔记 4.3/4.5 节），
//! 用 bluer 的 rfcomm Stream 按已知 channel（RFCOMM_CHANNEL=5）直连，
//! 无需注册 BlueZ Profile（POC 的 dbus Profile 注册仅 Python 侧需要）。

use bluer::rfcomm::{SocketAddr as RfcommSocketAddr, Stream};
use bluer::Address;

use super::errors::BleError;
use crate::protocol::auth::Session;
use crate::protocol::consts::RFCOMM_CHANNEL;

pub struct Manager {
    stream: Option<Stream>,
    session: Option<Session>,
}

impl Manager {
    pub fn new() -> Self {
        Self { stream: None, session: None }
    }

    /// 建立 SPP 连接（RFCOMM ch5）。
    pub async fn connect(&mut self, address: &str) -> Result<(), BleError> {
        let addr: Address = address
            .parse()
            .map_err(|_| BleError::ConnectFailed(format!("无效蓝牙地址: {address}")))?;
        let stream = Stream::connect(RfcommSocketAddr::new(addr, RFCOMM_CHANNEL))
            .await
            .map_err(|e| BleError::ConnectFailed(e.to_string()))?;
        self.stream = Some(stream);
        Ok(())
    }

    /// 返回底层流引用（供协议层读写帧）。
    pub fn stream(&self) -> Result<&Stream, BleError> {
        self.stream.as_ref().ok_or_else(|| BleError::ConnectFailed("未连接".into()))
    }

    /// 可变引用（协议层需要 split 或改写）。
    pub fn stream_mut(&mut self) -> Result<&mut Stream, BleError> {
        self.stream.as_mut().ok_or_else(|| BleError::ConnectFailed("未连接".into()))
    }

    /// 关闭连接并释放句柄。
    pub async fn disconnect(&mut self) {
        if let Some(s) = self.stream.take() {
            // Stream 被 drop 时自动 shutdown（shutdown_on_drop=true）
            drop(s);
        }
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// 保存认证会话（供安装使用）。
    pub fn set_session(&mut self, session: Session) {
        self.session = Some(session);
    }

    /// 取认证会话；未认证返回错误。
    pub fn session(&self) -> Result<&Session, BleError> {
        self.session
            .as_ref()
            .ok_or_else(|| BleError::AuthFailed("尚未认证（请先连接并输入 authkey）".into()))
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}
