//! Android 版蓝牙连接层（JNI 桥到 Kotlin RFCOMM）。
//!
//! 架构：Kotlin `BleRfcomm.connect(addr)` 创建 RFCOMM socket 并返回底层 fd；
//! Rust 通过 JNI 调用，用 fd 包装成 tokio 字节流（AsyncRead/AsyncWrite），
//! 复用 Linux 版协议层（认证/安装/列表确认/存储查询）。
//!
//! 传输实现注意：BluetoothSocket 的 fd 是**阻塞模式**，不适合直接套 tokio AsyncFd
//! （AsyncFd 要求非阻塞 fd，阻塞 read 会卡住 tokio 线程导致 timeout 失效）。
//! 方案：spawn 一个阻塞读线程，libc::read 循环 + mpsc channel 送数据；
//! 写路径直接 libc::write（协议层逐批等 ACK，写窗口小，不会长期阻塞）。
//!
//! fd 所有权：fd 归 Kotlin BluetoothSocket/ParcelFileDescriptor 所有，Rust **只借用**
//! （绝不 close，否则 fdsan abort：'expected to be unowned, actually owned by ParcelFileDescriptor'）。
//! 断开由 Kotlin 侧 BleRfcomm.close() 关闭 socket 完成。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use jni::objects::{GlobalRef, JClass, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use super::errors::BleError;
use crate::protocol::auth::Session as AuthSession;

/// 全局 JavaVM（由 JNI 入口 Java_com_minstall_app_BleRfcomm_initJni 保存）。
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// 应用 ClassLoader（initJni 在 Java 线程执行时可获取，native 线程 FindClass 拿不到应用类）。
static APP_CLASSLOADER: OnceLock<GlobalRef> = OnceLock::new();

/// 加载应用类（如 com.minstall.app.BleScan）。
///
/// 坑（两重）：
/// 1. JNI `FindClass` 在 native 线程（spawn_blocking 等）只能访问系统类加载器，
///    找不到应用自己的类；
/// 2. `FindClass` 失败会在 JVM 上留下 pending exception，若不清除，后续 JNI 调用
///    全被挡住，且线程 detach 时未处理异常直接 FATAL EXCEPTION 闪退。
/// 解决：native 线程不试 FindClass，直接用 initJni 缓存的 ClassLoader.loadClass。
pub fn find_app_class<'local>(
    env: &mut JNIEnv<'local>,
    name: &str,
) -> Result<JClass<'local>, BleError> {
    // 防御：清掉任何残留 pending exception（如之前 FindClass 失败留下的）
    let _ = env.exception_clear();

    let loader = APP_CLASSLOADER.get().ok_or_else(|| {
        BleError::ConnectFailed("ClassLoader 未缓存（BleRfcomm.init 未调用）".into())
    })?;
    let jname = env
        .new_string(name.replace('/', "."))
        .map_err(|e| BleError::ConnectFailed(format!("JNI 字符串失败: {e}")))?;
    let jname_obj: JObject = jname.into();
    let class_obj = env
        .call_method(
            loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&jname_obj)],
        )
        .map_err(|e| {
            // loadClass 失败（类不存在等）同样会留 pending exception，必须清除，
            // 否则线程 detach 时闪退
            let _ = env.exception_clear();
            BleError::ConnectFailed(format!("JNI loadClass 失败: {e}"))
        })?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI loadClass 返回失败: {e}")))?;
    Ok(JClass::from(class_obj))
}

/// 供 scanner_android 等模块复用 JavaVM。
pub fn java_vm_ref() -> Option<&'static JavaVM> {
    JAVA_VM.get()
}

/// 由 Kotlin BleRfcomm.init() 调用：保存 JavaVM + 应用 ClassLoader 供后续 JNI 调用。
///
/// 注意：此函数在 Java 线程执行（Kotlin 侧调用），此时 FindClass 可用，
/// 能拿到应用 ClassLoader；native 线程拿不到，需这里缓存。
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_minstall_app_BleRfcomm_initJni(
    mut env: JNIEnv,
    class: JClass,
) {
    if let Ok(vm) = env.get_java_vm() {
        let _ = JAVA_VM.set(vm);
    }
    if let Ok(loader_val) = env.call_method(
        class,
        "getClassLoader",
        "()Ljava/lang/ClassLoader;",
        &[],
    ) {
        if let Ok(loader_obj) = loader_val.l() {
            if let Ok(gref) = env.new_global_ref(loader_obj) {
                let cached = APP_CLASSLOADER.set(gref).is_ok();
                println!("[minstall-jni] ClassLoader 缓存 {}", if cached { "成功" } else { "重复调用（已缓存）" });
            } else {
                println!("[minstall-jni] ClassLoader global ref 失败");
            }
        } else {
            println!("[minstall-jni] getClassLoader 返回值解析失败");
        }
    } else {
        println!("[minstall-jni] getClassLoader 调用失败");
    }
}

/// Android RFCOMM 字节流：阻塞读线程 + channel + 直接写 fd。
/// fd 归 Kotlin 所有，此处只借用（不 close，drop 时仅 shutdown 唤醒读线程）。
pub struct AndroidStream {
    /// 读数据通道（阻塞读线程 → 协议层）。收到空 Vec 表示连接断开/EOF。
    rx: mpsc::Receiver<Vec<u8>>,
    /// 写用 fd（借用，不拥有）。
    write_fd: i32,
    /// 读线程停止标记。
    stop: Arc<AtomicBool>,
    /// 读线程句柄（drop 时 join）。
    reader: Option<JoinHandle<()>>,
}

impl AndroidStream {
    fn from_fd(fd: i32) -> Result<Self, BleError> {
        if fd < 0 {
            return Err(BleError::ConnectFailed("Android 蓝牙 fd 无效".into()));
        }
        let write_fd = fd;

        let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                // 阻塞读（BluetoothSocket fd 为阻塞模式，符合预期：无数据时挂起）
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                if n > 0 {
                    if tx.blocking_send(buf[..n as usize].to_vec()).is_err() {
                        break; // 接收端已 drop
                    }
                } else if n == 0 {
                    // EOF：通知断开
                    let _ = tx.blocking_send(Vec::new());
                    break;
                } else {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    // 其他错误（含 shutdown 唤醒）视为断开
                    let _ = tx.blocking_send(Vec::new());
                    break;
                }
            }
            // 注意：不 close fd（所有权在 Kotlin BluetoothSocket）
        });
        Ok(Self { rx, write_fd, stop, reader: Some(reader) })
    }

    /// 调 Kotlin BleRfcomm.close() 关闭 socket（fd 归 Kotlin，由它正常 close）。
    pub fn kotlin_close() {
        if let Some(vm) = JAVA_VM.get() {
            if let Ok(mut env) = vm.attach_current_thread() {
                let _ = env.exception_clear();
                if let Ok(class) = find_app_class(&mut env, "com/minstall/app/BleRfcomm") {
                    let _ = env.call_static_method(class, "close", "()V", &[]);
                    let _ = env.exception_clear();
                }
            }
        }
    }

    /// JNI 调 Kotlin BleRfcomm.connect(addr)，返回 fd。
    fn jni_connect(address: &str) -> Result<i32, BleError> {
        let vm = JAVA_VM
            .get()
            .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
        let class = find_app_class(&mut env, "com/minstall/app/BleRfcomm")?;
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

impl Drop for AndroidStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 唤醒阻塞读线程：shutdown（不 close，fd 归 Kotlin）让阻塞 read 立即返回错误/0。
        unsafe {
            libc::shutdown(self.write_fd, libc::SHUT_RDWR);
        }
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

impl AsyncRead for AndroidStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // 从读线程 channel 取数据；空 Vec = EOF/断开（read 返回 0）
        match self.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(data)) => {
                if data.is_empty() {
                    return std::task::Poll::Ready(Ok(())); // EOF → 上层读到 0 字节
                }
                buf.put_slice(&data);
                std::task::Poll::Ready(Ok(()))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(Ok(())), // 通道关闭 = 断开
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl AsyncWrite for AndroidStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        // 直接写 fd。阻塞模式下若内核缓冲满会阻塞，但协议层逐批等 ACK（BATCH=2），
        // 写窗口小，实测不会长期阻塞；且这里不阻塞 tokio 线程太久。
        let n = unsafe { libc::write(self.write_fd, buf.as_ptr() as *const _, buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                return std::task::Poll::Ready(Ok(0));
            }
            return std::task::Poll::Ready(Err(e));
        }
        std::task::Poll::Ready(Ok(n as usize))
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
        // 先让 Kotlin 关闭 socket（fd 归 Kotlin，Rust 不 close），再 drop 流唤醒读线程
        if self.stream.is_some() {
            AndroidStream::kotlin_close();
        }
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
