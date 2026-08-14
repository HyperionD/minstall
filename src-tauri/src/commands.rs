//! Tauri command 桥接：前端调用 → ble/protocol 层。
//!
//! Manager 以 Arc<Mutex<Manager>> 形式共享（跨 command），认证会话保存在 Manager 中供安装使用。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::ble::connection::Manager;
use crate::ble::scanner;
use crate::protocol::{auth, watchface};

pub type SharedManager = Arc<Mutex<Manager>>;

pub fn shared_manager() -> SharedManager {
    Arc::new(Mutex::new(Manager::new()))
}

/// 手环存储信息（供前端显示）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub used: u64,
    pub total: u64,
}

#[tauri::command]
pub async fn scan_devices() -> Result<Vec<scanner::DeviceInfo>, String> {
    scanner::scan(10).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn connect(state: State<'_, SharedManager>, address: String) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    mgr.connect(&address).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn disconnect(state: State<'_, SharedManager>) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    mgr.disconnect().await;
    Ok(())
}

#[tauri::command]
pub async fn authenticate(state: State<'_, SharedManager>, authkey: String) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    let stream = mgr.stream_mut().map_err(|e| e.to_string())?;
    let session = auth::authenticate(stream, &authkey, None)
        .await
        .map_err(|e| e.to_string())?;
    // 认证后 seq 从 2 起（0=PhoneNonce, 1=AuthStep3），供后续发送复用
    mgr.advance_seq(session.seq);
    mgr.set_session(session);
    Ok(())
}

/// 查询手环存储使用（需已连接并认证）。
#[tauri::command]
pub async fn get_storage_info(
    state: State<'_, SharedManager>,
) -> Result<StorageInfo, String> {
    let mut mgr = state.inner().lock().await;
    let session = match mgr.session() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[minstall] get_storage_info: 未认证 {e}");
            return Err(e.to_string());
        }
    };
    let mut seq = mgr.seq();
    let stream = match mgr.stream_mut() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[minstall] get_storage_info: 无连接 {e}");
            return Err(e.to_string());
        }
    };
    eprintln!("[minstall] get_storage_info: 开始查询 (seq={seq})");
    match watchface::query_storage(stream, &session, &mut seq).await {
        Ok((used, total)) => {
            mgr.advance_seq(seq);
            eprintln!("[minstall] get_storage_info: used={used} total={total}");
            Ok(StorageInfo { used, total })
        }
        Err(e) => {
            eprintln!("[minstall] get_storage_info: 失败 {e}");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn install_watchface(
    app: AppHandle,
    state: State<'_, SharedManager>,
    bin_path: String,
) -> Result<watchface::PushOutcome, String> {
    let mut mgr = state.inner().lock().await;
    let session = mgr.session().map_err(|e| e.to_string())?;
    let mut seq = mgr.seq();
    let stream = mgr.stream_mut().map_err(|e| e.to_string())?;
    // 读取表盘字节：Android 走 JNI（SAF 持久授权 URI 或路径），Linux 直接 fs::read
    #[cfg(target_os = "android")]
    let data = {
        let name = bin_path.clone();
        tokio::task::spawn_blocking(move || crate::ble::file_picker_android::read_bytes(&name))
            .await
            .map_err(|e| format!("读取表盘任务失败: {e}"))?
            .map_err(|e| e.to_string())?
    };
    #[cfg(not(target_os = "android"))]
    let data = std::fs::read(&bin_path).map_err(|e| format!("读取表盘文件失败: {e}"))?;

    let result = watchface::push(stream, &session, data, &mut seq, |sent, total| {
        let r = app.emit(
            "install:progress",
            serde_json::json!({ "sent": sent, "total": total }),
        );
        eprintln!("[minstall] emit install:progress sent={sent} total={total} ok={}", r.is_ok());
    })
    .await;
    mgr.advance_seq(seq);
    result.map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
#[tauri::command]
pub async fn pick_watchface_file() -> Result<String, String> {
    let path = tokio::task::spawn_blocking(|| crate::ble::file_picker_android::pick())
        .await
        .map_err(|e| format!("文件选择任务失败: {e}"))?
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Android authkey 自动读取（剪贴板 + wearablelog 日志）。
/// 返回状态码："FOUND|<hex>" / "DIR_MISSING" / "NEED_PERMISSION" / "EMPTY"。
#[cfg(target_os = "android")]
#[tauri::command]
pub async fn read_authkey() -> Result<String, String> {
    let val = tokio::task::spawn_blocking(|| crate::ble::authkey_android::read())
        .await
        .map_err(|e| format!("authkey 读取任务失败: {e}"))?
        .map_err(|e| e.to_string())?;
    eprintln!("[minstall] read_authkey 返回: '{val}'");
    Ok(val)
}

/// 打开系统「所有文件访问」设置页（MANAGE_EXTERNAL_STORAGE，Android 11+）。
#[cfg(target_os = "android")]
#[tauri::command]
pub async fn open_storage_permission_settings() -> Result<(), String> {
    tokio::task::spawn_blocking(|| crate::ble::authkey_android::open_storage_settings())
        .await
        .map_err(|e| format!("打开设置页任务失败: {e}"))?
        .map_err(|e| e.to_string())
}
