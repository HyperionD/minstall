//! authkey 认证握手（V2 协议，SPP 通道，PROTOBUF 通道）。
//!
//! 流程来源 docs/protocol-notes.md 4.3/4.4/4.5 节（真机验证）：
//! V1 Hello → START_SESSION → PhoneNonce → WatchNonce(HMAC 验证) → AuthStep3 → 认证完成。
//! 认证成功后返回 Session（加密通道密钥），供 watchface 安装使用。

use crate::ble::errors::BleError;
use crate::protocol::consts::*;
use crate::protocol::encoding::*;

/// 认证成功后的会话材料（协议笔记 4.4 节：deriveSession 输出 64B）。
#[derive(Debug, Clone)]
pub struct Session {
    pub enc_key: [u8; 16],
    pub dec_key: [u8; 16],
    pub enc_nonce4: [u8; 4],
    /// 下一个可用 seq（PhoneNonce=0, AuthStep3=1，之后从 2 起）
    pub seq: u8,
}

/// AuthDeviceInfo 参数（桌面端默认值，参考实现字段）。
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub unknown1: u32,
    pub phone_api_level: f32,
    pub phone_name: String,
    pub unknown3: u32,
    pub region: String,
}

impl Default for DeviceInfo {
    fn default() -> Self {
        Self {
            unknown1: 0,
            phone_api_level: 34.0,
            phone_name: hostname_str(),
            unknown3: 224,
            region: region_str(),
        }
    }
}

/// 主机名（与 POC platform.node() 一致；手环可能据此区分客户端）。
fn hostname_str() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "PC".to_string())
}

/// 地区码（与 POC 从 LC_ALL/LANG 推导一致，如 zh_CN → CN）。
fn region_str() -> String {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let parts: Vec<&str> = lang.split('.').next().unwrap_or("").split('_').collect();
    if parts.len() >= 2 && !parts[1].is_empty() {
        let p = parts[1];
        p[..p.len().min(2)].to_uppercase()
    } else {
        "CN".to_string()
    }
}

/// 等待某帧满足条件；自动回 ACK。超时/断开返回错误。
/// 注意：底层 read 可能永久阻塞，必须用 timeout 包裹，每次至多等 200ms。
async fn wait_for<S, F>(
    ch: &mut SppChannel<'_, S>,
    predicate: F,
    label: &str,
    timeout_secs: u64,
) -> Result<(u8, u8, Vec<u8>), BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: Fn(u8, u8, &[u8]) -> bool,
{
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(BleError::AuthFailed(format!("等待 {label} 超时")));
        }
        // 单次读取至多等 200ms（read 无数据时永久阻塞，必须限时）
        match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return Err(BleError::AuthFailed(format!("等待 {label} 期间 SPP 断开"))),
            Ok(Err(e)) => return Err(BleError::ConnectFailed(e)),
            Err(_) => continue, // 无数据，回到循环检查 deadline
        }
        for (pt, seq, payload) in ch.drain_ack().await.map_err(BleError::ConnectFailed)? {
            if predicate(pt, seq, &payload) {
                return Ok((pt, seq, payload));
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
}

/// 执行完整认证握手。成功返回 Session（加密通道密钥 + 后续 seq）。
/// S 为传输层（tokio AsyncRead+AsyncWrite）：Linux bluer Stream / Android JNI 桥。
pub async fn authenticate<S>(
    stream: &mut S,
    authkey_hex: &str,
    device_info: Option<&DeviceInfo>,
) -> Result<Session, BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let secret = parse_authkey(authkey_hex)
        .ok_or_else(|| BleError::AuthFailed(format!("authkey 应为 {AUTHKEY_LEN} hex 字符（可带 0x 前缀）")))?;

    let mut ch = SppChannel::new(stream);
    let di = device_info.cloned().unwrap_or_default();

    // 1) V1 Hello（协议笔记 4.3 节：SPP 连接后必需前置）
    ch.write(V1_HELLO).await.map_err(BleError::ConnectFailed)?;
    log_stub("→ V1 Hello");

    // 2) START_SESSION_REQUEST（seq 固定 0）
    let sess_frame = build_session_config(1);
    ch.write(&sess_frame).await.map_err(BleError::ConnectFailed)?;
    log_stub("→ START_SESSION_REQUEST");
    wait_for(
        &mut ch,
        |pt, _seq, payload| pt == V2_PACKET_SESSION_CONFIG && payload.first() == Some(&2),
        "START_SESSION_RESPONSE",
        15,
    )
    .await?;

    // 3) PhoneNonce（明文，seq=0）
    let phone_nonce = rand_16();
    let cmd = encode_command_phone_nonce(&phone_nonce);
    let pn_frame = build_protobuf_frame(0, &cmd, false, &[]);
    ch.write(&pn_frame).await.map_err(BleError::ConnectFailed)?;
    log_stub(&format!("→ PhoneNonce {}", hex_str(&phone_nonce)));

    // 4) 等 WatchNonce（type=1, subtype=26, 含 watchNonce）并验证 HMAC
    let watch = wait_for(
        &mut ch,
        |pt, _seq, payload| {
            if pt != V2_PACKET_DATA || payload.len() < 2 || payload[0] & 0x0F != CHANNEL_PROTOBUF {
                return false;
            }
            matches!(parse_command(&payload[2..]), Some(c) if c.typ == Some(1) && c.subtype == Some(26) && c.watch_nonce.is_some())
        },
        "WatchNonce",
        10,
    )
    .await?;
    let watch_payload = &watch.2;
    let cmd = parse_command(&watch_payload[2..])
        .ok_or_else(|| BleError::AuthFailed("WatchNonce 解析失败".into()))?;
    let watch_nonce = cmd.watch_nonce.ok_or_else(|| BleError::AuthFailed("WatchNonce 缺 nonce".into()))?;
    let watch_hmac = cmd.watch_hmac.ok_or_else(|| BleError::AuthFailed("WatchNonce 缺 hmac".into()))?;
    if watch_nonce.len() != 16 {
        return Err(BleError::AuthFailed(format!("WatchNonce 长度异常: {}", watch_nonce.len())));
    }

    let mut wnonce16 = [0u8; 16];
    wnonce16.copy_from_slice(&watch_nonce);
    let derived = derive_session(&secret, &phone_nonce, &wnonce16);
    let dec_key: [u8; 16] = derived[0..16].try_into().unwrap();
    let enc_key: [u8; 16] = derived[16..32].try_into().unwrap();
    let enc_nonce4: [u8; 4] = derived[36..40].try_into().unwrap();

    if !verify_watch_hmac(&dec_key, &wnonce16, &phone_nonce, &watch_hmac) {
        return Err(BleError::AuthFailed("watch HMAC 验证失败（authkey 可能不正确）".into()));
    }
    log_stub("watch HMAC 验证通过");

    // 5) AuthStep3（明文，seq=1；encryptV1 counter=0）
    let info_bytes = encode_auth_device_info(
        di.unknown1,
        di.phone_api_level,
        di.phone_name.as_bytes(),
        di.unknown3,
        di.region.as_bytes(),
    );
    let step3 = encode_command_auth_step3(
        &phone_ack(&enc_key, &phone_nonce, &wnonce16),
        &encrypt_v1(&enc_key, &enc_nonce4, 0, &info_bytes),
    );
    let s3_frame = build_protobuf_frame(1, &step3, false, &[]);
    ch.write(&s3_frame).await.map_err(BleError::ConnectFailed)?;
    log_stub("→ AuthStep3");

    // 6) 等认证完成（type=1, subtype=27 或 5）
    wait_for(
        &mut ch,
        |pt, _seq, payload| {
            if pt != V2_PACKET_DATA || payload.len() < 2 || payload[0] & 0x0F != CHANNEL_PROTOBUF {
                return false;
            }
            matches!(parse_command(&payload[2..]), Some(c) if c.typ == Some(1) && matches!(c.subtype, Some(27) | Some(5)))
        },
        "认证完成",
        10,
    )
    .await?;
    log_stub("认证成功");

    Ok(Session { enc_key, dec_key, enc_nonce4, seq: 2 })
}

// ---- 小工具（无 rand 依赖：用 /dev/urandom，回退时间戳混合）----
use std::io::Read;

fn rand_16() -> [u8; 16] {
    let mut out = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut out).is_ok() {
            return out;
        }
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = t.as_nanos().to_le_bytes();
    for (i, b) in nanos.iter().enumerate() {
        out[i % 16] ^= b;
    }
    out
}

fn hex_str(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn log_stub(msg: &str) {
    eprintln!("[minstall] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authkey_validation_matches_poc() {
        assert!(parse_authkey("abababababababababababababababab").is_some());
        assert!(parse_authkey("0xabababababababababababababababab").is_some());
        assert!(parse_authkey("ababababababababababababababab").is_none());
        assert!(parse_authkey("zzababababababababababababababab").is_none());
    }

    #[test]
    fn golden_flow_frames_build() {
        // PhoneNonce 帧：V2 DATA + channel=1 + op=1 + Command{1,26}
        let pn = [0u8; 16];
        let frame = build_protobuf_frame(0, &encode_command_phone_nonce(&pn), false, &[]);
        let parsed = parse_v2_frame(&frame).unwrap().unwrap();
        assert_eq!(parsed.0, V2_PACKET_DATA);
        assert_eq!(parsed.2[0] & 0x0F, CHANNEL_PROTOBUF);
        assert_eq!(parsed.2[1], OPCODE_PLAINTEXT);
        let cmd = parse_command(&parsed.2[2..]).unwrap();
        assert_eq!(cmd.typ, Some(1));
        assert_eq!(cmd.subtype, Some(26));
        assert_eq!(cmd.phone_nonce, Some(vec![0u8; 16]));
    }

    #[test]
    fn session_config_frame_matches_golden() {
        let frame = build_session_config(1);
        let parsed = parse_v2_frame(&frame).unwrap().unwrap();
        assert_eq!(parsed.0, V2_PACKET_SESSION_CONFIG);
        assert_eq!(parsed.2[0], 1); // opcode START_SESSION_REQUEST
    }

    #[test]
    fn rand_16_produces_unique() {
        assert_ne!(rand_16(), rand_16());
    }
}
