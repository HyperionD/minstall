//! Tauri command 桥接：前端调用 → ble/protocol 层。
//!
//! Manager 以 Arc<Mutex<Manager>> 形式共享（跨 command），认证会话保存在 Manager 中供安装使用。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

use crate::ble::connection::Manager;
use crate::ble::scanner;
use crate::protocol::{auth, watchface};

pub type SharedManager = Arc<Mutex<Manager>>;

pub fn shared_manager() -> SharedManager {
    Arc::new(Mutex::new(Manager::new()))
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
    mgr.set_session(session);
    Ok(())
}

#[tauri::command]
pub async fn install_watchface(
    app: AppHandle,
    state: State<'_, SharedManager>,
    bin_path: String,
) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    let session = mgr.session().map_err(|e| e.to_string())?.clone();
    watchface::push(&mut mgr, &session, &bin_path, |sent, total| {
        let _ = app.emit(
            "install:progress",
            serde_json::json!({ "sent": sent, "total": total }),
        );
    })
    .await
    .map_err(|e| e.to_string())
}
