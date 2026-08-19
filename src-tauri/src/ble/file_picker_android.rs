//! Android SAF 文件选择 JNI 桥。
//! 选择器为非阻塞启动，结果通过持久邮箱按 request_id 查询。

use jni::objects::{JObject, JString, JValue};

use super::errors::BleError;
use super::file_picker::{parse_picker_result, PickerResult};

fn request_id_to_jlong(request_id: u64) -> Result<i64, BleError> {
    i64::try_from(request_id)
        .map_err(|_| BleError::FileError("文件选择请求 ID 超出 Android 支持范围".into()))
}

pub fn launch(request_id: u64) -> Result<(), BleError> {
    let vm = super::connection_android::java_vm_ref().ok_or_else(|| {
        BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into())
    })?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| BleError::ConnectFailed(format!("JNI attach 失败: {error}")))?;
    let class =
        super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let request_id = request_id_to_jlong(request_id)?;
    let started = env
        .call_static_method(class, "launch", "(J)Z", &[JValue::Long(request_id)])
        .map_err(|error| BleError::FileError(format!("启动文件选择器失败: {error}")))?
        .z()
        .map_err(|error| BleError::FileError(format!("读取文件选择启动状态失败: {error}")))?;
    if !started {
        return Err(BleError::FileError("已有文件选择请求进行中".into()));
    }
    Ok(())
}

pub fn get_result(request_id: u64) -> Result<PickerResult, BleError> {
    let vm = super::connection_android::java_vm_ref().ok_or_else(|| {
        BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into())
    })?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| BleError::ConnectFailed(format!("JNI attach 失败: {error}")))?;
    let class =
        super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let request_id = request_id_to_jlong(request_id)?;
    let result = env
        .call_static_method(
            class,
            "getResult",
            "(J)Ljava/lang/String;",
            &[JValue::Long(request_id)],
        )
        .map_err(|error| BleError::FileError(format!("查询文件选择结果失败: {error}")))?
        .l()
        .map_err(|error| BleError::FileError(format!("读取文件选择结果失败: {error}")))?;
    if result.is_null() {
        return Ok(PickerResult::Missing);
    }
    let raw: String = env
        .get_string(&JString::from(result))
        .map_err(|error| BleError::FileError(format!("转换文件选择结果失败: {error}")))?
        .into();
    parse_picker_result(&raw).map_err(BleError::FileError)
}

pub fn clear_result(request_id: u64) -> Result<(), BleError> {
    let vm = super::connection_android::java_vm_ref().ok_or_else(|| {
        BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into())
    })?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| BleError::ConnectFailed(format!("JNI attach 失败: {error}")))?;
    let class =
        super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let request_id = request_id_to_jlong(request_id)?;
    env.call_static_method(class, "clearResult", "(J)V", &[JValue::Long(request_id)])
        .map_err(|error| BleError::FileError(format!("清理文件选择结果失败: {error}")))?;
    Ok(())
}

/// 按文件名读取持久授权 URI；失败返回空 Vec。
pub fn read_bytes(name: &str) -> Result<Vec<u8>, BleError> {
    let vm = super::connection_android::java_vm_ref().ok_or_else(|| {
        BleError::ConnectFailed("JavaVM 未初始化（BleRfcomm.init 未调用）".into())
    })?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| BleError::ConnectFailed(format!("JNI attach 失败: {error}")))?;
    let class =
        super::connection_android::find_app_class(&mut env, "com/minstall/app/BleFilePicker")?;
    let name = env
        .new_string(name)
        .map_err(|error| BleError::FileError(format!("转换文件名失败: {error}")))?;
    let name: JObject = name.into();
    let result = env
        .call_static_method(
            class,
            "readBytes",
            "(Ljava/lang/String;)[B",
            &[JValue::Object(&name)],
        )
        .map_err(|error| BleError::FileError(format!("读取所选文件失败: {error}")))?
        .l()
        .map_err(|error| BleError::FileError(format!("读取文件字节数组失败: {error}")))?;
    if result.is_null() {
        return Ok(Vec::new());
    }
    let bytes: jni::objects::JByteArray = result.into();
    env.convert_byte_array(&bytes)
        .map_err(|error| BleError::FileError(format!("转换文件字节失败: {error}")))
}
