//! Vela 快应用 RPK 包解析与安装协议。

use std::io::{Cursor, Read};

use serde::Deserialize;
use zip::ZipArchive;

use crate::ble::errors::BleError;
use crate::protocol::auth::Session;
use crate::protocol::consts::*;
use crate::protocol::encoding::{
    build_protobuf_frame, encode_mass_prepare_with_type, encode_v2_frame, field_bytes,
    field_varint, parse_proto_fields, parse_wear_packet, ProtoVal, SppChannel, WearPacket,
};
use crate::protocol::watchface::{self, PushOutcome};

const MAX_MANIFEST_SIZE: u64 = 1024 * 1024;
// AstroBox 的 Xiaomi ThirdpartyApp 安装实现使用该非零版本标记；
// manifest.versionCode 仍用于展示和日志，不作为协议安装标记。
const INSTALL_VERSION_MARKER: u32 = 114_514;

#[derive(Debug, Clone, PartialEq)]
pub struct QuickAppInfo {
    pub package: String,
    pub name: String,
    pub version_name: String,
    pub version_code: u32,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    package: String,
    name: String,
    #[serde(rename = "versionName")]
    version_name: String,
    #[serde(rename = "versionCode")]
    version_code: u32,
    #[serde(rename = "deviceTypeList")]
    device_type_list: Vec<String>,
}

pub fn encode_install_request(package: &str, version_code: u32, package_size: usize) -> Vec<u8> {
    let mut install = field_bytes(1, package.as_bytes());
    install.extend_from_slice(&field_varint(2, version_code as u64));
    install.extend_from_slice(&field_varint(3, package_size as u64));

    let thirdparty = field_bytes(2, &install);
    let mut packet = field_varint(1, WEARPACKET_TYPE_THIRDPARTY_APP as u64);
    packet.extend_from_slice(&field_varint(2, WP_ID_PREPARE_INSTALL_APP as u64));
    packet.extend_from_slice(&field_bytes(
        WEARPACKET_PAYLOAD_THIRDPARTY_APP as u64,
        &thirdparty,
    ));
    packet
}

pub(crate) fn parse_quick_app_packages(data: &[u8]) -> Vec<String> {
    let fields = match parse_proto_fields(data) {
        Ok(fields) => fields,
        Err(_) => return vec![],
    };
    let is_list = fields.iter().any(|(num, value)| {
        (*num == 1 && *value == ProtoVal::Varint(WEARPACKET_TYPE_THIRDPARTY_APP as u64))
            || (*num == 2 && *value == ProtoVal::Varint(0))
    });
    if !is_list {
        return vec![];
    }

    let thirdparty = fields.iter().find_map(|(num, value)| {
        (*num == WEARPACKET_PAYLOAD_THIRDPARTY_APP as u64).then_some(value)
    });
    let Some(ProtoVal::Bytes(thirdparty)) = thirdparty else {
        return vec![];
    };
    let thirdparty = match parse_proto_fields(thirdparty) {
        Ok(fields) => fields,
        Err(_) => return vec![],
    };
    let Some(ProtoVal::Bytes(list)) = thirdparty
        .iter()
        .find_map(|(num, value)| (*num == 1).then_some(value))
    else {
        return vec![];
    };
    let Ok(list) = parse_proto_fields(list) else {
        return vec![];
    };

    list.iter()
        .filter_map(|(num, value)| {
            if *num != 1 {
                return None;
            }
            let ProtoVal::Bytes(app) = value else {
                return None;
            };
            let app = parse_proto_fields(app).ok()?;
            app.iter().find_map(|(app_num, app_value)| {
                (*app_num == 1).then(|| match app_value {
                    ProtoVal::Bytes(package) => String::from_utf8(package.clone()).ok(),
                    ProtoVal::Varint(_) => None,
                })
            })?
        })
        .collect()
}

pub fn parse_rpk(data: &[u8]) -> Result<QuickAppInfo, BleError> {
    let mut archive = ZipArchive::new(Cursor::new(data))
        .map_err(|error| BleError::FileError(format!("无效的 RPK ZIP 包: {error}")))?;
    let mut file = archive
        .by_name("manifest.json")
        .map_err(|error| BleError::FileError(format!("RPK 缺少 manifest.json: {error}")))?;
    if file.size() > MAX_MANIFEST_SIZE {
        return Err(BleError::FileError("RPK manifest.json 过大".into()));
    }

    let mut manifest_data = String::new();
    file.read_to_string(&mut manifest_data)
        .map_err(|error| BleError::FileError(format!("读取 RPK manifest.json 失败: {error}")))?;
    let manifest: Manifest = serde_json::from_str(&manifest_data)
        .map_err(|error| BleError::FileError(format!("解析 RPK manifest.json 失败: {error}")))?;
    if manifest.package.trim().is_empty() || manifest.name.trim().is_empty() {
        return Err(BleError::FileError(
            "RPK manifest 缺少应用包名或名称".into(),
        ));
    }
    if manifest.version_name.trim().is_empty() {
        return Err(BleError::FileError("RPK manifest 缺少版本名称".into()));
    }
    if !manifest.device_type_list.iter().any(|kind| kind == "watch") {
        return Err(BleError::FileError("RPK 不是 watch 快应用".into()));
    }

    Ok(QuickAppInfo {
        package: manifest.package,
        name: manifest.name,
        version_name: manifest.version_name,
        version_code: manifest.version_code,
    })
}

async fn send_encrypted<S>(
    channel: &mut SppChannel<'_, S>,
    session: &Session,
    sequence: &mut u8,
    body: &[u8],
) -> Result<(), BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = build_protobuf_frame(*sequence, body, true, &session.enc_key);
    *sequence = sequence.wrapping_add(1);
    channel.write(&frame).await.map_err(BleError::ConnectFailed)
}

async fn wait_for_packet<S, F>(
    channel: &mut SppChannel<'_, S>,
    session: &Session,
    predicate: F,
    label: &str,
    timeout_secs: u64,
) -> Result<WearPacket, BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: Fn(&WearPacket) -> bool,
{
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(BleError::PushFailed {
                chunk: 0,
                detail: format!("等待 {label} 超时"),
            });
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), channel.read_more()).await
        {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                return Err(BleError::PushFailed {
                    chunk: 0,
                    detail: format!("等待 {label} 期间 SPP 断开"),
                });
            }
            Ok(Err(error)) => return Err(BleError::ConnectFailed(error)),
            Err(_) => continue,
        }
        for (packet_type, _, payload) in
            channel.drain_ack().await.map_err(BleError::ConnectFailed)?
        {
            if packet_type != V2_PACKET_DATA {
                continue;
            }
            if let Some(body) = watchface::protobuf_body(&payload, session) {
                if let Some(packet) = parse_wear_packet(&body) {
                    eprintln!(
                        "[minstall] 快应用响应 typ={:?} id={:?} prepare={:?} slice={:?} result={:?}",
                        packet.typ,
                        packet.id,
                        packet.prepare_status,
                        packet.slice_length,
                        packet.install_result_code,
                    );
                    if predicate(&packet) {
                        return Ok(packet);
                    }
                }
            }
        }
    }
}

async fn query_installed_packages<S>(
    channel: &mut SppChannel<'_, S>,
    session: &Session,
    sequence: &mut u8,
) -> Vec<String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut request = field_varint(1, WEARPACKET_TYPE_THIRDPARTY_APP as u64);
    request.extend_from_slice(&field_varint(2, 0));
    if send_encrypted(channel, session, sequence, &request)
        .await
        .is_err()
    {
        eprintln!("[minstall] 快应用列表请求发送失败");
        return vec![];
    }

    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() >= deadline {
            eprintln!("[minstall] 快应用列表查询超时");
            return vec![];
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), channel.read_more()).await
        {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) | Ok(Err(_)) => return vec![],
            Err(_) => continue,
        }
        let frames = match channel.drain_ack().await {
            Ok(frames) => frames,
            Err(_) => return vec![],
        };
        for (packet_type, _, payload) in frames {
            if packet_type != V2_PACKET_DATA {
                continue;
            }
            let Some(body) = watchface::protobuf_body(&payload, session) else {
                continue;
            };
            let packages = parse_quick_app_packages(&body);
            if !packages.is_empty() {
                eprintln!("[minstall] 快应用列表: {packages:?}");
                return packages;
            }
        }
    }
}

async fn send_mass<S>(
    channel: &mut SppChannel<'_, S>,
    data: &[u8],
    sequence: &mut u8,
    slice_length: usize,
    on_progress: impl Fn(usize, usize),
) -> Result<(), BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (frames, total, framed) =
        watchface::build_mass_frames_with_type(data, slice_length, MASS_DATA_TYPE_THIRDPARTY_APP);
    let batch = std::env::var("MINSTALL_MASS_BATCH")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(MASS_BATCH);
    let mut index = 0;
    while index < total {
        let end = (index + batch).min(total);
        for (part, payload) in frames.iter().enumerate().take(end).skip(index) {
            let frame = encode_v2_frame(V2_PACKET_DATA, *sequence, payload);
            channel
                .write(&frame)
                .await
                .map_err(|error| BleError::PushFailed {
                    chunk: part,
                    detail: error,
                })?;
            *sequence = sequence.wrapping_add(1);
        }
        watchface::drain_until_ack(channel, sequence.wrapping_sub(1)).await?;
        index = end;
        on_progress((framed.len() * index / total).min(data.len()), data.len());
    }
    Ok(())
}

pub async fn push<S>(
    stream: &mut S,
    session: &Session,
    data: Vec<u8>,
    sequence: &mut u8,
    on_progress: impl Fn(usize, usize),
) -> Result<PushOutcome, BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if data.is_empty() {
        return Err(BleError::FileError("文件为空".into()));
    }
    let info = parse_rpk(&data)?;
    eprintln!(
        "[minstall] rpk: package={} name={:?} version={} code={} size={}",
        info.package,
        info.name,
        info.version_name,
        info.version_code,
        data.len()
    );

    let mut channel = SppChannel::new(stream);
    send_encrypted(
        &mut channel,
        session,
        sequence,
        &encode_install_request(&info.package, INSTALL_VERSION_MARKER, data.len()),
    )
    .await?;
    let prepare = wait_for_packet(
        &mut channel,
        session,
        |packet| {
            packet.typ == Some(WEARPACKET_TYPE_THIRDPARTY_APP)
                && packet.id == Some(WP_ID_PREPARE_INSTALL_APP)
                && packet.prepare_status.is_some()
        },
        "快应用安装准备响应",
        30,
    )
    .await?;
    if prepare.prepare_status != Some(0) {
        return Err(BleError::PushFailed {
            chunk: 0,
            detail: format!("快应用安装准备失败: {:?}", prepare.prepare_status),
        });
    }

    let digest = watchface::md5(&data);
    send_encrypted(
        &mut channel,
        session,
        sequence,
        &encode_mass_prepare_with_type(&digest, data.len() as u32, MASS_DATA_TYPE_THIRDPARTY_APP),
    )
    .await?;
    let mass = wait_for_packet(
        &mut channel,
        session,
        |packet| {
            packet.typ == Some(WEARPACKET_TYPE_MASS)
                && packet.id == Some(WP_ID_MASS_PREPARE)
                && packet.prepare_status.is_some()
        },
        "MASS 准备响应",
        30,
    )
    .await?;
    if mass.prepare_status != Some(0) {
        return Err(BleError::PushFailed {
            chunk: 0,
            detail: format!("MASS 准备失败: {:?}", mass.prepare_status),
        });
    }

    send_mass(
        &mut channel,
        &data,
        sequence,
        mass.slice_length.unwrap_or(DEFAULT_SLICE_LENGTH),
        on_progress,
    )
    .await?;
    let result = wait_for_packet(
        &mut channel,
        session,
        |packet| {
            packet.typ == Some(WEARPACKET_TYPE_THIRDPARTY_APP)
                && packet.id == Some(WP_ID_REPORT_INSTALL_APP_RESULT)
                && packet.install_result_code.is_some()
        },
        "快应用安装结果",
        10,
    )
    .await;
    match result {
        Ok(packet) => match packet.install_result_code {
            Some(0) => Ok(PushOutcome::Confirmed),
            Some(code) => Err(BleError::PushFailed {
                chunk: 0,
                detail: format!("快应用安装失败，设备返回 code={code}"),
            }),
            None => Err(BleError::PushFailed {
                chunk: 0,
                detail: "快应用安装结果缺少 code".into(),
            }),
        },
        Err(error) => {
            eprintln!("[minstall] 未收到快应用安装结果: {error}");
            let packages = query_installed_packages(&mut channel, session, sequence).await;
            if packages.iter().any(|package| package == &info.package) {
                eprintln!("[minstall] 快应用列表确认已安装: {}", info.package);
                Ok(PushOutcome::Confirmed)
            } else {
                eprintln!("[minstall] 快应用列表未确认安装: {}", info.package);
                Ok(PushOutcome::Transferred)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::{encode_install_request, parse_quick_app_packages, parse_rpk};
    use crate::protocol::consts::{
        MASS_DATA_TYPE_THIRDPARTY_APP, WEARPACKET_PAYLOAD_THIRDPARTY_APP,
        WEARPACKET_TYPE_THIRDPARTY_APP,
    };
    use crate::protocol::encoding::{
        encode_mass_prepare_with_type, field_bytes, field_varint, parse_proto_fields,
        parse_wear_packet, ProtoVal,
    };
    use crate::protocol::watchface::build_mass_frames_with_type;

    fn build_rpk(manifest: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        writer
            .start_file(
                "manifest.json",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn encodes_thirdparty_app_install_request() {
        let encoded = encode_install_request("com.example.test", 7, 42);

        assert_eq!(
            encoded,
            vec![
                0x08, 0x14, 0x10, 0x01, 0xb2, 0x01, 0x18, 0x12, 0x16, 0x0a, 0x10, b'c', b'o', b'm',
                b'.', b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b't', b'e', b's', b't', 0x10,
                0x07, 0x18, 0x2a,
            ]
        );
    }

    #[test]
    fn parses_installed_quick_app_packages() {
        let app = field_bytes(1, b"com.application.watch.demo");
        let list = field_bytes(1, &app);
        let thirdparty = field_bytes(1, &list);
        let mut packet = field_varint(1, WEARPACKET_TYPE_THIRDPARTY_APP as u64);
        packet.extend_from_slice(&field_varint(2, 0));
        packet.extend_from_slice(&field_bytes(
            WEARPACKET_PAYLOAD_THIRDPARTY_APP as u64,
            &thirdparty,
        ));

        assert_eq!(
            parse_quick_app_packages(&packet),
            vec!["com.application.watch.demo"]
        );
    }

    #[test]
    fn quick_app_mass_prepare_uses_thirdparty_data_type() {
        let encoded = encode_mass_prepare_with_type(&[0; 16], 42, MASS_DATA_TYPE_THIRDPARTY_APP);
        let outer = parse_proto_fields(&encoded).unwrap();
        let mass = outer
            .iter()
            .find_map(|(num, value)| (*num == 24).then_some(value))
            .and_then(|value| match value {
                ProtoVal::Bytes(value) => Some(parse_proto_fields(value).unwrap()),
                ProtoVal::Varint(_) => None,
            })
            .unwrap();
        let request = mass
            .iter()
            .find_map(|(num, value)| (*num == 1).then_some(value))
            .and_then(|value| match value {
                ProtoVal::Bytes(value) => Some(parse_proto_fields(value).unwrap()),
                ProtoVal::Varint(_) => None,
            })
            .unwrap();
        let data_type = request.iter().find_map(|(num, value)| {
            (*num == 1).then_some(match value {
                ProtoVal::Varint(value) => *value,
                ProtoVal::Bytes(_) => 0,
            })
        });

        assert_eq!(data_type, Some(MASS_DATA_TYPE_THIRDPARTY_APP as u64));
    }

    #[test]
    fn quick_app_mass_frames_use_thirdparty_data_type() {
        let (frames, _, _) = build_mass_frames_with_type(b"rpk", 64, MASS_DATA_TYPE_THIRDPARTY_APP);

        assert_eq!(frames[0][7], MASS_DATA_TYPE_THIRDPARTY_APP);
    }

    #[test]
    fn parses_quick_app_prepare_and_result() {
        let response = field_varint(1, 0);
        let response = {
            let mut value = response;
            value.extend_from_slice(&field_varint(2, 12_288));
            value
        };
        let mut thirdparty = field_bytes(3, &response);
        thirdparty.extend_from_slice(&field_bytes(4, &field_varint(1, 0)));
        let mut packet = field_varint(1, 20);
        packet.extend_from_slice(&field_varint(2, 1));
        packet.extend_from_slice(&field_bytes(22, &thirdparty));

        let parsed = parse_wear_packet(&packet).unwrap();

        assert_eq!(parsed.typ, Some(20));
        assert_eq!(parsed.id, Some(1));
        assert_eq!(parsed.prepare_status, Some(0));
        assert_eq!(parsed.slice_length, Some(12_288));
        assert_eq!(parsed.install_result_code, Some(0));
    }

    #[test]
    fn parses_watch_quick_app_manifest() {
        let rpk = build_rpk(
            r#"{
                "package": "com.application.watch.demo",
                "name": "存储空间",
                "versionName": "1.0.0",
                "versionCode": 1,
                "deviceTypeList": ["watch"],
                "minAPILevel": 1
            }"#,
        );

        let info = parse_rpk(&rpk).unwrap();

        assert_eq!(info.package, "com.application.watch.demo");
        assert_eq!(info.name, "存储空间");
        assert_eq!(info.version_name, "1.0.0");
        assert_eq!(info.version_code, 1);
    }

    #[test]
    fn rejects_rpk_without_manifest() {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        writer
            .start_file("app.js", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"console.log('test')").unwrap();
        let rpk = writer.finish().unwrap().into_inner();

        let error = parse_rpk(&rpk).unwrap_err().to_string();

        assert!(error.contains("manifest.json"));
    }

    #[test]
    fn rejects_manifest_for_non_watch_device() {
        let rpk = build_rpk(
            r#"{
                "package": "com.example.phone",
                "name": "手机应用",
                "versionName": "1.0.0",
                "versionCode": 1,
                "deviceTypeList": ["phone"]
            }"#,
        );

        let error = parse_rpk(&rpk).unwrap_err().to_string();

        assert!(error.contains("watch"));
    }
}
