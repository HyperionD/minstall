//! Android 版蓝牙连接层（JNI 桥到 Kotlin RFCOMM）。
//!
//! 架构：Kotlin 负责 RFCOMM socket 连接，通过 JNI 把 fd 传给 Rust，
//! Rust 用 tokio AsyncFd 包装成 AsyncRead/AsyncWrite，复用协议层。
//!
//! 当前为骨架：连接层逐步实现（先保证 APK 可编译，再接入 JNI）。

use tokio::io::{AsyncRead, AsyncWrite};

use super::errors::BleError;
use crate::protocol::auth::Session as AuthSession;

/// Android 字节流（JNI fd → tokio）。连接后由 connect 填充。
pub struct AndroidStream;

impl AsyncRead for AndroidStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "Android 蓝牙流未连接",
        )))
    }
}

impl AsyncWrite for AndroidStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "Android 蓝牙流未连接",
        )))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// 连接管理器（Android）。会话/seq 管理平台无关，连接走 Kotlin JNI。
pub struct Manager {
    stream: Option<AndroidStream>,
    session: Option<AuthSession>,
    seq: u8,
}

impl Manager {
    pub fn new() -> Self {
        Self { stream: None, session: None, seq: 0 }
    }

    /// 建立 RFCOMM 连接（JNI 桥到 Kotlin）。当前骨架：待实现 JNI。
    pub async fn connect(&mut self, address: &str) -> Result<(), BleError> {
        let _ = address;
        Err(BleError::ConnectFailed(
            "Android 蓝牙层开发中（JNI 桥待实现）".into(),
        ))
    }

    pub async fn disconnect(&mut self) {
        self.stream.take();
        self.session = None;
    }

    pub fn stream_mut(&mut self) -> Result<&mut AndroidStream, BleError> {
        self.stream
            .as_mut()
            .ok_or_else(|| BleError::ConnectFailed("未连接".into()))
    }

    pub fn set_session(&mut self, session: AuthSession) {
        self.session = Some(session);
    }

    pub fn session(&self) -> Result<AuthSession, BleError> {
        self.session
            .clone()
            .ok_or_else(|| BleError::AuthFailed("尚未认证（请先连接并输入 authkey）".into()))
    }

    pub fn seq(&self) -> u8 {
        self.seq
    }

    pub fn advance_seq(&mut self, next: u8) {
        self.seq = next;
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}
