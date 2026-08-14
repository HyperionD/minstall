//! 表盘安装：bin 解析 + WearPacket 安装流程（MASS 分块上传）。
//!
//! 流程与帧格式来源 docs/protocol-notes.md 5 节（真机验证，2026-08-12）：
//! WatchFace PREPARE_INSTALL → Mass PREPARE → MASS 分片上传（BATCH=2）→ 等 InstallResult。

use std::fs;

use crate::ble::errors::BleError;
use crate::protocol::auth::Session;
use crate::protocol::consts::*;
use crate::protocol::encoding::*;

/// 取字节串前 n 字节的 hex（诊断日志用）。
pub fn hex_prefix(data: &[u8], n: usize) -> String {
    data.iter()
        .take(n)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 把数据按 size 切块（纯函数，测试契约同 POC chunk_data）。
pub fn chunk_data(data: &[u8], size: usize) -> Vec<&[u8]> {
    data.chunks(size).collect()
}

/// 从表盘 bin 提取 id（offset 0x28 起 null-terminated ASCII，须匹配 `^\d+$`；协议笔记 6 节）。
pub fn parse_watchface_id(data: &[u8]) -> Result<String, BleError> {
    if data.len() < 0x40 || data[0] != 0x5A || data[1] != 0xA5 {
        return Err(BleError::FileError("非法表盘文件：头部 magic 应为 5A A5".into()));
    }
    let start = 0x28usize;
    let end = data[start..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| start + p)
        .unwrap_or_else(|| (start + 32).min(data.len()));
    let id_bytes = &data[start..end];
    let id_str = std::str::from_utf8(id_bytes)
        .map_err(|_| BleError::FileError(format!("表盘 id 非 ASCII: {:?}", &id_bytes[..id_bytes.len().min(16)])))?;
    if !id_str.chars().all(|c| c.is_ascii_digit()) {
        return Err(BleError::FileError(format!("表盘 id 应为数字: {id_str:?}")));
    }
    Ok(id_str.to_string())
}

/// 提取表盘名称（offset 0x68 起 null-terminated；0x68 处为 0xFFFFFFFF 时走 i18n 表；协议笔记 6 节）。
pub fn parse_watchface_name(data: &[u8]) -> String {
    if data.len() < 0x7C {
        return String::new();
    }
    let at = &data[0x68..0x6C];
    if at == [0xFF; 4] {
        let i18n_off = u32::from_le_bytes(data[0x74..0x78].try_into().unwrap()) as usize;
        let i18n_size = u32::from_le_bytes(data[0x78..0x7C].try_into().unwrap()) as usize;
        if i18n_off + i18n_size <= data.len() {
            let tbl = &data[i18n_off..i18n_off + i18n_size];
            if let Some(end) = tbl.iter().position(|&b| b == 0) {
                return String::from_utf8_lossy(&tbl[..end]).into_owned();
            }
        }
        return String::new();
    }
    let end = data[0x68..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| 0x68 + p)
        .unwrap_or_else(|| (0x68 + 64).min(data.len()));
    String::from_utf8_lossy(&data[0x68..end]).into_owned()
}

/// 构造 MASS 分片（纯函数，同 POC build_mass_frames）。
/// 返回 (frames, total_parts, with_crc)。
pub fn build_mass_frames(data: &[u8], slice_length: usize) -> (Vec<Vec<u8>>, usize, Vec<u8>) {
    // MD5（16B）用于 MassPacket data_id；crc32 用于尾部校验，均手写实现（见下文）。
    let md5 = md5(data);

    let mut framed = vec![0x00u8]; // comp_data 版本 0
    framed.push(MASS_DATA_TYPE);
    framed.extend_from_slice(&md5);
    framed.extend_from_slice(&(data.len() as u32).to_le_bytes());
    framed.extend_from_slice(data);
    let mut with_crc = framed.clone();
    with_crc.extend_from_slice(&crc32(&framed).to_le_bytes());

    let fragment_max = slice_length.saturating_sub(MASS_FRAME_OVERHEAD).max(64);
    let total_parts = with_crc.len().div_ceil(fragment_max);
    let mut frames = Vec::with_capacity(total_parts);
    for i in 0..total_parts {
        let start = i * fragment_max;
        let end = ((i + 1) * fragment_max).min(with_crc.len());
        let fragment = &with_crc[start..end];
        let mut header = Vec::with_capacity(6 + fragment.len());
        header.push(CHANNEL_DATA); // channel=2 (Mass, 明文)
        header.push(1);            // op=1 (Write)
        header.extend_from_slice(&(total_parts as u16).to_le_bytes());
        header.extend_from_slice(&((i + 1) as u16).to_le_bytes()); // cur 从 1 起
        header.extend_from_slice(fragment);
        frames.push(header);
    }
    (frames, total_parts, with_crc)
}

// ---------------------------------------------------------------------------
// MD5（RFC 1321 实现，16 字节输出；MassPacket data_id 要求）
// ---------------------------------------------------------------------------

fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
        14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
        21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8,
        0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340,
        0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87,
        0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039,
        0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92,
        0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let mut msg = data.to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, word) in chunk.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes(word.try_into().unwrap());
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(S[i]),
            );
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// CRC32（IEEE，MassPacket 尾部校验）
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 };
        }
    }
    !crc
}

/// 从 Data 帧 payload 提取 PROTOBUF 通道明文（若为加密则解密）。
fn protobuf_body(payload: &[u8], session: &Session) -> Option<Vec<u8>> {
    if payload.len() < 2 {
        return None;
    }
    let chan = payload[0] & 0x0F;
    let op = payload[1];
    let body = &payload[2..];
    if chan != CHANNEL_PROTOBUF {
        return None;
    }
    match op {
        OPCODE_PLAINTEXT => Some(body.to_vec()),
        OPCODE_ENCRYPTED => Some(decrypt_v2(&session.dec_key, body)),
        _ => None,
    }
}

/// 安装结果：反馈给用户。传输成功即返回，不长时间等待确认（体验优先）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PushOutcome {
    /// InstallResult 明确确认（code=2/3）或列表确认。
    Confirmed,
    /// 字节流已全部传输完成，但手环未推确认（正常现象，需用户在手环上确认）。
    Transferred,
}

/// 安装表盘：认证后调用（stream 已连 SPP，seq 由调用方维护）。on_progress(sent_bytes, total_bytes)。
/// S 为传输层（tokio AsyncRead+AsyncWrite）：Linux bluer Stream / Android JNI 桥。
pub async fn push<S>(
    stream: &mut S,
    session: &Session,
    bin_path: &str,
    seq_ref: &mut u8,
    on_progress: impl Fn(usize, usize),
) -> Result<PushOutcome, BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let data = fs::read(bin_path).map_err(|e| BleError::FileError(e.to_string()))?;
    if data.is_empty() {
        return Err(BleError::FileError("文件为空".into()));
    }
    let watchface_id = parse_watchface_id(&data)?;
    let watchface_name = parse_watchface_name(&data);
    eprintln!(
        "[minstall] bin: id={watchface_id} name={watchface_name:?} size={}",
        data.len()
    );

    let mut seq = *seq_ref; // 局部 u8 序列号（内部递增，结束写回 seq_ref）
    let mut ch = SppChannel::new(stream);

    // 发送加密 WearPacket 帧
    async fn send_enc<S>(
        ch: &mut SppChannel<'_, S>,
        session: &Session,
        seq: &mut u8,
        wp: &[u8],
    ) -> Result<(), BleError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let frame = build_protobuf_frame(*seq, wp, true, &session.enc_key);
        *seq = seq.wrapping_add(1);
        ch.write(&frame).await.map_err(BleError::ConnectFailed)
    }

    // 等待 WearPacket 应答（加密通道）
    async fn wait_wp<'x, S, F>(
        ch: &mut SppChannel<'x, S>,
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
                return Err(BleError::PushFailed { chunk: 0, detail: format!("等待 {label} 超时") });
            }
            // 单次读取至多等 200ms（read 无数据时永久阻塞，必须限时）
            match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => {
                    return Err(BleError::PushFailed { chunk: 0, detail: format!("等待 {label} 期间 SPP 断开") })
                }
                Ok(Err(e)) => return Err(BleError::ConnectFailed(e)),
                Err(_) => continue, // 无数据，回到循环检查 deadline
            }
            for (pt, _fseq, payload) in ch.drain_ack().await.map_err(BleError::ConnectFailed)? {
                if pt == V2_PACKET_DATA {
                    let body = protobuf_body(&payload, session);
                    if let Some(body) = body {
                        if let Some(wp) = parse_wear_packet(&body) {
                            if predicate(&wp) {
                                return Ok(wp);
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }

    // 1) WatchFace PREPARE_INSTALL（加密通道）
    send_enc(&mut ch, session, &mut seq, &encode_watchface_prepare(&watchface_id, data.len() as u32)).await?;
    eprintln!("[minstall] → WatchFace PREPARE_INSTALL id={watchface_id} size={}", data.len());
    let wp = wait_wp(
        &mut ch,
        session,
        |w| {
            w.typ == Some(WEARPACKET_TYPE_WATCH_FACE)
                && w.id == Some(WP_ID_PREPARE_INSTALL_WATCH_FACE)
                && w.prepare_status.is_some()
        },
        "watchface prepare_reply",
        15,
    )
    .await?;
    if wp.prepare_status != Some(0) {
        return Err(BleError::PushFailed { chunk: 0, detail: format!("WatchFace prepare_status={:?}（非 READY）", wp.prepare_status) });
    }

    // 2) Mass PREPARE（加密通道，data_id=md5）
    let md5 = md5(&data);
    send_enc(&mut ch, session, &mut seq, &encode_mass_prepare(&md5, data.len() as u32)).await?;
    eprintln!("[minstall] → Mass PREPARE type=16 size={}", data.len());
    let wp = wait_wp(
        &mut ch,
        session,
        |w| {
            w.typ == Some(WEARPACKET_TYPE_MASS)
                && w.id == Some(WP_ID_MASS_PREPARE)
                && w.prepare_status.is_some()
        },
        "mass prepare_response",
        15,
    )
    .await?;
    if wp.prepare_status != Some(0) {
        return Err(BleError::PushFailed { chunk: 0, detail: format!("Mass prepare_status={:?}（非 READY）", wp.prepare_status) });
    }
    let slice_length = wp.slice_length.unwrap_or(DEFAULT_SLICE_LENGTH);
    eprintln!("[minstall] expected_slice_length={slice_length}");

    // 3) MASS 分片上传（channel=2 Mass, op=1 Write）
    //    批量窗口：默认 MASS_BATCH（18，快传 ~30s）；逐批等 ACK 保证数据完整。
    //    可通过环境变量 MINSTALL_MASS_BATCH 覆盖（诊断用）。
    let batch = std::env::var("MINSTALL_MASS_BATCH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|b| *b >= 1)
        .unwrap_or(MASS_BATCH);
    eprintln!("[minstall] MASS batch={batch}");
    let (frames, total, with_crc) = build_mass_frames(&data, slice_length);
    let data_seq_start = seq;
    let mut data_seq = data_seq_start;
    let mut idx = 0usize;
    let total_bytes = data.len();
    let t_upload = std::time::Instant::now();
    let mut ack_waits: Vec<std::time::Duration> = Vec::new();

    while idx < total {
        let batch_end = (idx + batch).min(total);
        for j in idx..batch_end {
            let frame = encode_v2_frame(V2_PACKET_DATA, data_seq, &frames[j]);
            ch.write(&frame).await.map_err(|e| BleError::PushFailed { chunk: j, detail: e })?;
            data_seq = data_seq.wrapping_add(1);
        }
        // 等这批最后一块的 ACK（必须等到，否则手环丢数据）
        let last_seq = data_seq.wrapping_sub(1);
        let t_ack = std::time::Instant::now();
        drain_until_ack(&mut ch, last_seq).await?;
        ack_waits.push(t_ack.elapsed());
        idx = batch_end;
        let sent = (with_crc.len() * idx / total).min(total_bytes);
        on_progress(sent, total_bytes);
        eprintln!("[minstall] MASS {idx}/{total} ({sent}B) ack_wait={:?}", t_ack.elapsed());
    }
    eprintln!("[minstall] MASS 上传完成 {total} 块 耗时 {:?}", t_upload.elapsed());
    seq = data_seq; // 上传后的下一个 seq（结尾统一写回 seq_ref）
    let avg_wait = if ack_waits.is_empty() {
        0.0
    } else {
        ack_waits.iter().map(|d| d.as_secs_f64()).sum::<f64>() / ack_waits.len() as f64
    };
    eprintln!("[minstall] 平均每批 ACK 等待 {avg_wait:.3}s（共 {} 批）", ack_waits.len());

    // 4) 确认安装结果：等手环推送 InstallResult（id=5，code 2/3 成功）短时间；
    //    手环经常不推（真机多次验证），故仅短等待，未收到即返回「已传输」，
    //    由用户在手环上确认，避免长时间卡等待（体验优先）。
    let install = wait_wp(
        &mut ch,
        session,
        |w| {
            w.typ == Some(WEARPACKET_TYPE_WATCH_FACE)
                && w.id == Some(WP_ID_REPORT_INSTALL_RESULT)
                && w.install_result_code.is_some()
        },
        "InstallResult",
        10, // 手环经常不推 InstallResult；只短等 10s，未收到即返回已传输
    )
    .await;
    let install = match install {
        Ok(wp) => Some(wp),
        Err(_) => None,
    };
    if let Some(wp) = &install {
        let code = wp.install_result_code.unwrap_or(0);
        if code == INSTALL_RESULT_SUCCESS || code == INSTALL_RESULT_USED {
            eprintln!("[minstall] ★ InstallResult code={code}（2=SUCCESS, 3=INSTALL_USED）");
            *seq_ref = seq;
            return Ok(PushOutcome::Confirmed);
        }
        eprintln!("[minstall] InstallResult code={code}（非成功）");
    } else {
        eprintln!("[minstall] InstallResult 10s 未收到（手环常不推），不再等待");
    }
    // 快速查一次列表：能确认就报已安装，查不到也不阻塞（返回已传输）
    let wp_id = parse_watchface_id(&data)?;
    let ids = query_installed_ids(&mut ch, session, &mut seq).await;
    eprintln!("[minstall] 列表查询：{} 个表盘", ids.len());
    let found = ids.iter().any(|id| id == &wp_id);
    if found {
        eprintln!("[minstall] ★ 表盘列表确认：{wp_id} 已安装");
    } else {
        eprintln!("[minstall] 列表未见 {wp_id}（可能仍在写入），按已传输处理");
    }
    // seq 由调用方维护（已通过 &mut seq 递增），此处直接返回结果
    *seq_ref = seq; // 写回会话 seq（供后续查询/安装复用，避免 seq 冲突）
    if found {
        Ok(PushOutcome::Confirmed)
    } else {
        Ok(PushOutcome::Transferred)
    }
}

/// 发 GET_INSTALLED_LIST 并解析响应中的表盘 id 列表。
async fn query_installed_ids<S>(
    ch: &mut SppChannel<'_, S>,
    session: &Session,
    seq: &mut u8,
) -> Vec<String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = build_protobuf_frame(*seq, &encode_get_installed_list(), true, &session.enc_key);
    *seq = seq.wrapping_add(1);
    eprintln!("[minstall] GET_INSTALLED_LIST 发送 seq={}", seq.wrapping_sub(1));
    if ch.write(&frame).await.is_err() {
        eprintln!("[minstall] GET_INSTALLED_LIST 写入失败");
        return vec![];
    }
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    loop {
        if tokio::time::Instant::now() >= deadline {
            eprintln!("[minstall] GET_INSTALLED_LIST 等待响应超时");
            return vec![];
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) | Ok(Err(_)) => {
                eprintln!("[minstall] GET_INSTALLED_LIST 读取失败/断开");
                return vec![];
            }
            Err(_) => continue,
        }
        for (pt, _fseq, payload) in ch.drain_ack().await.unwrap_or_default() {
            if pt != V2_PACKET_DATA {
                continue;
            }
            if let Some(body) = protobuf_body(&payload, session) {
                if let Some(wp) = parse_wear_packet(&body) {
                    eprintln!("[minstall] 收到 WearPacket typ={:?} id={:?}", wp.typ, wp.id);
                    if wp.typ == Some(WEARPACKET_TYPE_WATCH_FACE) && wp.id == Some(WP_ID_GET_INSTALLED_LIST) {
                        let ids = parse_watchface_list(&body);
                        eprintln!("[minstall] 解析到 {} 个表盘: {:?}", ids.len(), ids);
                        eprintln!("[minstall] 原始响应 body hex (前 256B): {}", hex_prefix(&body, 256));
                        return ids;
                    }
                }
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
    }
}

/// 查询手环存储使用（GET_STORAGE_INFO）。返回 (used_bytes, total_bytes)。
/// 查询手环存储使用（GET_STORAGE_INFO）。返回 (used_bytes, total_bytes)。
pub async fn query_storage<S>(
    stream: &mut S,
    session: &Session,
    seq: &mut u8,
) -> Result<(u64, u64), BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut ch = SppChannel::new(stream);

    // GET_STORAGE_INFO: WearPacket{type=SYSTEM(2), id=62}，无 payload
    let pkt = {
        let mut out = field_varint(1, WEARPACKET_TYPE_SYSTEM as u64);
        out.extend_from_slice(&field_varint(2, WP_ID_GET_STORAGE_INFO as u64));
        out
    };
    let frame = build_protobuf_frame(*seq, &pkt, true, &session.enc_key);
    *seq = seq.wrapping_add(1);
    ch.write(&frame).await.map_err(BleError::ConnectFailed)?;

    // 等响应：WearPacket{type=2, id=62, System{storage_info=44{used=1, total=2}}}
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15);
    let mut result: Option<(u64, u64)> = None;
    while result.is_none() {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => break,
            Ok(Err(e)) => return Err(BleError::ConnectFailed(e)),
            Err(_) => continue,
        }
        for (pt, _fseq, payload) in ch.drain_ack().await.map_err(BleError::ConnectFailed)? {
            if pt != V2_PACKET_DATA {
                continue;
            }
            if let Some(body) = protobuf_body(&payload, session) {
                result = parse_storage_info(&body);
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
    }
    drop(ch);
    result.ok_or_else(|| BleError::PushFailed { chunk: 0, detail: "存储查询超时或响应异常".into() })
}

/// 解析存储响应：WearPacket{type=2, id=62} → System(field 4) → storage_info(field 44) → {used=1, total=2}。
fn parse_storage_info(body: &[u8]) -> Option<(u64, u64)> {
    let fields = parse_proto_fields(body).ok()?;
    for (num, val) in &fields {
        if *num == 4 {
            if let ProtoVal::Bytes(b) = val {
                let sys = parse_proto_fields(b).ok()?;
                for (sn, sv) in &sys {
                    if *sn == 44 {
                        if let ProtoVal::Bytes(si) = sv {
                            let info = parse_proto_fields(si).ok()?;
                            let mut used: u64 = 0;
                            let mut total: u64 = 0;
                            for (in_, iv) in &info {
                                match (in_, iv) {
                                    (1, ProtoVal::Varint(v)) => used = *v,
                                    (2, ProtoVal::Varint(v)) => total = *v,
                                    _ => {}
                                }
                            }
                            return Some((used, total));
                        }
                    }
                }
            }
        }
    }
    None
}

/// 读取直到收到指定 seq 的 ACK（期间回手环推送的 ACK）。
/// 必须等到 ACK 才能继续：手环处理慢（真机 ~18KB/s，BATCH=2 每批 ~1.3s），
/// 超时继续会导致手环丢数据（表盘损坏）。
async fn drain_until_ack<S>(
    ch: &mut SppChannel<'_, S>,
    seq_target: u8,
) -> Result<(), BleError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(BleError::PushFailed { chunk: 0, detail: format!("等待 seq={seq_target} ACK 超时") });
        }
        // 单次读取至多等 200ms（read 无数据时永久阻塞，必须限时）
        match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                return Err(BleError::PushFailed { chunk: 0, detail: "等待 ACK 期间 SPP 断开".into() })
            }
            Ok(Err(e)) => return Err(BleError::ConnectFailed(e)),
            Err(_) => continue,
        }
        let frames = ch.drain_ack().await.map_err(BleError::ConnectFailed)?;
        // 保留非 ACK 的 Data 帧（如 InstallResult 可能随 ACK 一起到达），
        // 匹配到目标 ACK 时重新塞回累积器供后续 wait_wp 读取——否则会被消费丢弃。
        let mut keep: Vec<(u8, u8, Vec<u8>)> = Vec::new();
        for (pt, seq, payload) in frames {
            if pt == V2_PACKET_ACK && seq == seq_target {
                ch.acc.requeue(&keep);
                eprintln!("[minstall] ACK seq={seq} 已收到");
                return Ok(());
            }
            if pt == V2_PACKET_DATA {
                keep.push((pt, seq, payload));
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_data_by_size() {
        let data = b"abcdef";
        let out = chunk_data(data, 2);
        assert_eq!(out, vec![b"ab".as_slice(), b"cd".as_slice(), b"ef".as_slice()]);
    }

    #[test]
    fn chunk_boundary_exact_multiple() {
        let data = b"abcd";
        assert_eq!(chunk_data(data, 2).len(), 2);
    }

    #[test]
    fn md5_known_vectors() {
        let md5_bytes = |d: &[u8]| -> Vec<u8> { md5(d).to_vec() };
        assert_eq!(md5_bytes(b""), hex_decode("d41d8cd98f00b204e9800998ecf8427e"));
        assert_eq!(md5_bytes(b"abc"), hex_decode("900150983cd24fb0d6963f7d28e17f72"));
        assert_eq!(
            md5_bytes(b"The quick brown fox jumps over the lazy dog"),
            hex_decode("9e107d9d372bb6826bd81d3542a419d6")
        );
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }

    #[test]
    fn mass_frames_reassemble() {
        let data = [0x5Au8, 0xA5].iter().chain(std::iter::repeat(&1u8).take(100)).copied().collect::<Vec<_>>();
        let (frames, total, with_crc) = build_mass_frames(&data, 64);
        assert_eq!(total, frames.len());
        assert!(total > 0);
        for (i, fr) in frames.iter().enumerate() {
            assert_eq!(fr[0], 2); // channel=2 Mass
            assert_eq!(fr[1], 1); // op=1 Write
            let t = u16::from_le_bytes([fr[2], fr[3]]);
            let cur = u16::from_le_bytes([fr[4], fr[5]]);
            assert_eq!(t as usize, total);
            assert_eq!(cur as usize, i + 1);
        }
        let merged: Vec<u8> = frames.iter().flat_map(|f| f[6..].iter().copied()).collect();
        assert_eq!(merged, with_crc);
        // 尾部 crc32
        let body = &with_crc[..with_crc.len() - 4];
        assert_eq!(u32::from_le_bytes(with_crc[with_crc.len() - 4..].try_into().unwrap()), crc32(body));
    }

    #[test]
    fn parse_watchface_id_and_name() {
        let mut fake = vec![0u8; 0x100];
        fake[0] = 0x5A;
        fake[1] = 0xA5;
        fake[0x28..0x28 + 6].copy_from_slice(b"12345\x00");
        assert_eq!(parse_watchface_id(&fake).unwrap(), "12345");
        fake[0x68..0x68 + 7].copy_from_slice(b"MyFace\x00");
        assert_eq!(parse_watchface_name(&fake), "MyFace");

        // 非法 magic
        let bad = vec![0u8; 0x100];
        assert!(parse_watchface_id(&bad).is_err());
        // 非数字 id
        let mut bad2 = fake.clone();
        bad2[0x28..0x28 + 5].copy_from_slice(b"ab123");
        assert!(parse_watchface_id(&bad2).is_err());
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
