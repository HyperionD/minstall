//! Android 版蓝牙连接层（JNI 桥到 Kotlin RFCOMM）。
//!
//! 架构：Kotlin `BleRfcomm.connect(addr)` 创建 RFCOMM socket 并返回底层 fd；
//! Rust 通过 JNI 调用，用 fd 包装成 tokio AsyncFd（AsyncRead/AsyncWrite），
//! 复用 Linux 版协议层（认证/安装/列表确认/存储查询）。

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::OnceLock;

use jni::objects::{JClass, JValue};
use jni::{JNIEnv, JavaVM};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::errors::BleError;
use crate::protocol::auth::Session as AuthSession;

/// 全局 JavaVM（由 JNI 入口 Java_com_minstall_app_BleRfcomm_initJni 保存）。
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// 由 Kotlin BleRfcomm.init() 调用：保存 JavaVM 供后续 JNI 调用。
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_minstall_app_BleRfcomm_initJni(
    mut env: JNIEnv,
    _: JClass,
) {
    if let Ok(vm) = env.get_java_vm() {
        let _ = JAVA_VM.set(vm);
    }
}

/// Android RFCOMM 字节流：fd → tokio AsyncFd。
pub struct AndroidStream {
    inner: AsyncFd<OwnedFd>,
}

impl AndroidStream {
    fn from_fd(fd: i32) -> Result<Self, BleError> {
        if fd < 0 {
            return Err(BleError::ConnectFailed("Android 蓝牙 fd 无效".into()));
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let afd = AsyncFd::new(owned)
            .map_err(|e| BleError::ConnectFailed(format!("AsyncFd 包装失败: {e}")))?;
        Ok(Self { inner: afd })
    }

    /// JNI 调 Kotlin BleRfcomm.connect(addr)，返回 fd。
    fn jni_connect(address: &str) -> Result<i32, BleError> {
        let vm = JAVA_VM
            .get()
            .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
        let class = env
            .find_class("com/minstall/app/BleRfcomm")
            .map_err(|e| BleError::ConnectFailed(format!("查找 BleRfcomm 失败: {e}")))?;
        let jaddr = env
            .new_string(address)
            .map_err(|e| BleError::ConnectFailed(format!("JNI 字符串失败: {e}")))?;
        let jaddr_obj: jni::objects::JObject = jaddr.into();
        let fd = env
            .call_static_method(
                class,
                "connect",
                "(Ljava/lang/String;)I",
                &[JValue::Object(&jaddr_obj)],
            )
            .map_err(|e| BleError::ConnectFailed(format!("JNI 调 connect 失败: {e}")))?
            .i()
            .map_err(|e| BleError::ConnectFailed(format!("JNI 返回值失败: {e}")))?;
        Ok(fd)
    }
}

impl AsyncRead for AndroidStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut guard = match self.inner.poll_read_ready(cx) {
            std::task::Poll::Ready(Ok(g)) => g,
            std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
            std::task::Poll::Pending => return std::task::Poll::Pending,
        };
        let res = guard.try_io(|inner| {
            let fd = inner.get_ref().as_raw_fd();
            let n = unsafe { libc::read(fd, buf.initialize_unfilled().as_mut_ptr() as *mut _, buf.remaining()) };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        });
        match res {
            Ok(Ok(n)) => {
                buf.advance(n);
                std::task::Poll::Ready(Ok(()))
            }
            Ok(Err(e)) => std::task::Poll::Ready(Err(e)),
            Err(_) => std::task::Poll::Pending, // WouldBlock，等下一次
        }
    }
}

impl AsyncWrite for AndroidStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let mut guard = match self.inner.poll_write_ready(cx) {
            std::task::Poll::Ready(Ok(g)) => g,
            std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
            std::task::Poll::Pending => return std::task::Poll::Pending,
        };
        let res = guard.try_io(|inner| {
            let fd = inner.get_ref().as_raw_fd();
            let n = unsafe { libc::write(fd, buf.as_ptr() as *const _, buf.len()) };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        });
        match res {
            Ok(Ok(n)) => std::task::Poll::Ready(Ok(n)),
            Ok(Err(e)) => std::task::Poll::Ready(Err(e)),
            Err(_) => std::task::Poll::Pending,
        }
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

    /// 建立 RFCOMM 连接（JNI 调 Kotlin）。需 Android 蓝牙权限（BLUETOOTH_CONNECT）。
    pub async fn connect(&mut self, address: &str) -> Result<(), BleError> {
        let fd = AndroidStream::jni_connect(address)?;
        let stream = AndroidStream::from_fd(fd)?;
        self.stream = Some(stream);
        Ok(())
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
