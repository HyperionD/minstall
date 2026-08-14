//! Android authkey 自动读取：JNI 调 Kotlin AuthkeyReader.read()。
//!
//! 数据源：剪贴板 32-hex 或小米运动健康日志文件（XiaomiFit.main.log）。
//! 返回 hex 字符串；找不到返回空串（前端提示手动输入）。

use super::errors::BleError;

/// 读取 authkey；找不到返回空串（不视为错误）。
pub fn read() -> Result<String, BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/AuthkeyReader")?;
    let val_obj = env
        .call_static_method(class, "read", "()Ljava/lang/String;", &[])
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 AuthkeyReader.read 失败: {e}")))?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 返回值失败: {e}")))?;
    let val: String = env
        .get_string(&jni::objects::JString::from(val_obj))
        .map(|j| j.into())
        .unwrap_or_default();
    Ok(val)
}

/// 打开系统「所有文件访问」设置页（Android 11+ MANAGE_EXTERNAL_STORAGE）。
pub fn open_storage_settings() -> Result<(), BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/AuthkeyReader")?;
    env.call_static_method(class, "openStorageSettings", "()V", &[])
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 openStorageSettings 失败: {e}")))?;
    let _ = env.exception_clear();
    Ok(())
}
