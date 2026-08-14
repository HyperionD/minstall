//! Android 文件选择（SAF）：JNI 调 Kotlin BleFilePicker.pick()。
//!
//! Kotlin 侧用系统文档选择器（ACTION_OPEN_DOCUMENT）选表盘文件，复制到 app 缓存目录，
//! 返回本地绝对路径；Rust 协议层按普通路径读取。取消/失败返回空串。

use super::errors::BleError;

/// 调 Kotlin BleFilePicker.pick()，返回本地文件路径；取消/失败返回空串。
pub fn pick() -> Result<String, BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let path_obj = env
        .call_static_method(class, "pick", "()Ljava/lang/String;", &[])
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 pick 失败: {e}")))?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 返回值失败: {e}")))?;
    let path: String = env
        .get_string(&jni::objects::JString::from(path_obj))
        .map(|j| j.into())
        .unwrap_or_default();
    eprintln!("[minstall] BleFilePicker.pick 返回: '{}' (len={})", path, path.len());
    Ok(path)
}
