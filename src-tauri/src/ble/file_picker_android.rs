//! Android 文件选择（SAF）：JNI 调 Kotlin BleFilePicker。
//!
//! - `pick()`：系统文档选择器选表盘文件，持久授权 URI，返回**原始文件名**（不复制缓存）。
//! - `read_bytes(name)`：按文件名读字节（持久授权 URI 或普通路径），供协议层安装。

use jni::objects::JValue;

use super::errors::BleError;

/// 调 Kotlin BleFilePicker.pick()，返回原始文件名；取消/失败返回空串。
pub fn pick() -> Result<String, BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let name_obj = env
        .call_static_method(class, "pick", "()Ljava/lang/String;", &[])
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 pick 失败: {e}")))?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 返回值失败: {e}")))?;
    let name: String = env
        .get_string(&jni::objects::JString::from(name_obj))
        .map(|j| j.into())
        .unwrap_or_default();
    eprintln!("[minstall] BleFilePicker.pick 返回: '{name}' (len={})", name.len());
    Ok(name)
}

/// 按文件名读取表盘字节（持久授权 URI 或普通路径）。失败返回空 Vec。
pub fn read_bytes(name: &str) -> Result<Vec<u8>, BleError> {
    let vm = super::connection_android::java_vm_ref()
        .ok_or_else(|| BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BleError::ConnectFailed(format!("JNI attach 失败: {e}")))?;
    let class = super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let jname = env
        .new_string(name)
        .map_err(|e| BleError::ConnectFailed(format!("JNI 字符串失败: {e}")))?;
    let jname_obj: jni::objects::JObject = jname.into();
    let arr = env
        .call_static_method(
            class,
            "readBytes",
            "(Ljava/lang/String;)[B",
            &[JValue::Object(&jname_obj)],
        )
        .map_err(|e| BleError::ConnectFailed(format!("JNI 调 readBytes 失败: {e}")))?
        .l()
        .map_err(|e| BleError::ConnectFailed(format!("JNI 返回数组失败: {e}")))?;
    if arr.is_null() {
        return Ok(Vec::new());
    }
    let arr_ref: jni::objects::JByteArray = jni::objects::JObject::from(arr).into();
    let len = env
        .get_array_length(&arr_ref)
        .map_err(|e| BleError::ConnectFailed(format!("数组长度失败: {e}")))?;
    let mut buf = vec![0i8; len as usize];
    env.get_byte_array_region(&arr_ref, 0, &mut buf)
        .map_err(|e| BleError::ConnectFailed(format!("读取字节失败: {e}")))?;
    Ok(buf.into_iter().map(|b| b as u8).collect())
}
