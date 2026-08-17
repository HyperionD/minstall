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

/// 读取 Android Keystore 中保存的 authkey；未保存时返回 None。
pub fn read_saved() -> Result<Option<String>, BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/AuthkeyReader")?;
    let val_obj = env
        .call_static_method(class, "readSaved", "()Ljava/lang/String;", &[])
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 AuthkeyReader.readSaved 失败: {e}")))?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 返回值失败: {e}")))?;
    let val: String = env
        .get_string(&jni::objects::JString::from(val_obj))
        .map(|j| j.into())
        .unwrap_or_default();
    Ok(crate::secure_storage::saved_value(&val))
}

/// 将 authkey 写入 Android Keystore 加密存储。
pub fn save_saved(value: &str) -> Result<(), BleError> {
    let normalized = crate::secure_storage::validate_authkey(value)
        .map_err(BleError::AuthFailed)?;
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/AuthkeyReader")?;
    let value_obj = env
        .new_string(normalized)
        .map_err(|e| BleError::ConnectFailed(format!("JNI 创建 authkey 字符串失败: {e}")))?;
    let saved = env
        .call_static_method(
            class,
            "saveSaved",
            "(Ljava/lang/String;)Z",
            &[(&value_obj).into()],
        )
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 AuthkeyReader.saveSaved 失败: {e}")))?
        .z()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 保存结果解析失败: {e}")))?;
    if saved {
        Ok(())
    } else {
        Err(BleError::ConnectFailed("Android Keystore 保存 authkey 失败".into()))
    }
}

/// 清除 Android Keystore 加密存储中的 authkey。
pub fn clear_saved() -> Result<(), BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/AuthkeyReader")?;
    let cleared = env
        .call_static_method(class, "clearSaved", "()Z", &[])
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 AuthkeyReader.clearSaved 失败: {e}")))?
        .z()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 清除结果解析失败: {e}")))?;
    if cleared {
        Ok(())
    } else {
        Err(BleError::ConnectFailed("清除 Android Keystore authkey 失败".into()))
    }
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
