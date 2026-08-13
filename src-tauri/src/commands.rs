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
    let session = auth::authenticate(&mut mgr, &authkey, None)
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
    eprintln!("[minstall] get_storage_info: 开始查询 (seq={})", mgr.seq());
    match watchface::query_storage(&mut mgr, &session).await {
        Ok((used, total)) => {
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
) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    let session = mgr.session().map_err(|e| e.to_string())?;
    watchface::push(&mut mgr, &session, &bin_path, |sent, total| {
        let _ = app.emit(
            "install:progress",
            serde_json::json!({ "sent": sent, "total": total }),
        );
    })
    .await
    .map_err(|e| e.to_string())
}
