//! 协议底层原语：V2 帧编解码、CRC-16/ARC、protobuf 编解码、AES/HMAC 认证算法、WearPacket 编码。
//!
//! 算法与字节布局的唯一来源 docs/protocol-notes.md 第 4/5 节（POC 真机验证），
//! 移植自 pocs/auth.py + pocs/install.py，测试用 POC golden 字节交叉验证。

use aes::Aes128;
use ccm::aead::{Aead, Payload};
use ccm::{consts::{U4, U12}, Ccm, Nonce};
use ctr::cipher::{KeyIvInit, StreamCipher};
use ctr::Ctr128BE;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::consts::*;

type HmacSha256 = Hmac<Sha256>;
type Aes128Ccm4 = Ccm<Aes128, U4, U12>; // tag 4B (macBits=32), nonce 12B

// ---------------------------------------------------------------------------
// CRC-16/ARC（协议笔记 4 节：poly 0x8005, init 0, 无 xor, refin, refout）
// ---------------------------------------------------------------------------

pub fn crc16_arc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// V2 帧编解码（协议笔记 4 节）
// ---------------------------------------------------------------------------

pub fn encode_v2_frame(packet_type: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(V2_HEADER_LEN + payload.len());
    out.extend_from_slice(&V2_PREAMBLE);
    out.push(packet_type & 0x0F);
    out.push(seq & 0xFF);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(&crc16_arc(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// 解析开头的单帧。返回 (packet_type, seq, payload)。
/// - 数据不足：Ok(None)（等待更多字节）
/// - preamble/CRC 非法：Err
pub fn parse_v2_frame(data: &[u8]) -> Result<Option<(u8, u8, Vec<u8>)>, String> {
    if data.len() < V2_HEADER_LEN {
        return Ok(None);
    }
    if data[0..2] != V2_PREAMBLE {
        return Err(format!("preamble 不匹配: {:02x?}", &data[0..2]));
    }
    let packet_type = data[2] & 0x0F;
    let seq = data[3];
    let payload_len = u16::from_le_bytes([data[4], data[5]]) as usize;
    let given_crc = u16::from_le_bytes([data[6], data[7]]);
    if data.len() < V2_HEADER_LEN + payload_len {
        return Ok(None);
    }
    let payload = data[V2_HEADER_LEN..V2_HEADER_LEN + payload_len].to_vec();
    if crc16_arc(&payload) != given_crc {
        return Err(format!("crc16 校验失败: 期望 {given_crc:#06x}"));
    }
    Ok(Some((packet_type, seq, payload)))
}

/// 字节流 → 完整 V2 帧序列（非法前缀重同步，同 POC V2Accumulator）。
#[derive(Default)]
pub struct V2Accumulator {
    buf: Vec<u8>,
}

impl V2Accumulator {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn feed(&mut self, data: &[u8]) {
        // 只累积不解析：帧由 drain() 统一提取（read_more → drain_ack 两次调用，
        // 若 feed 内部解析会双重消费导致帧丢失 —— 真机验证发现的 bug）。
        self.buf.extend_from_slice(data);
    }

    /// 便捷：feed + drain 一步完成（测试/一次性解析用）。
    pub fn feed_drain(&mut self, data: &[u8]) -> Vec<(u8, u8, Vec<u8>)> {
        self.feed(data);
        self.drain()
    }

    /// 取走当前缓冲中所有完整帧（不清空未完成部分）。
    pub fn drain(&mut self) -> Vec<(u8, u8, Vec<u8>)> {
        let mut frames = Vec::new();
        loop {
            match parse_v2_frame(&self.buf) {
                Ok(Some((pt, seq, payload))) => {
                    let consumed = V2_HEADER_LEN + payload.len();
                    self.buf.drain(..consumed);
                    frames.push((pt, seq, payload));
                }
                Ok(None) => break, // 不完整，等更多数据
                Err(_) => self.resync(), // 非法前缀，丢弃对齐
            }
        }
        frames
    }

    fn resync(&mut self) {
        match self.buf.windows(2).position(|w| w == V2_PREAMBLE) {
            Some(idx) if idx > 0 => {
                self.buf.drain(..idx);
            }
            Some(_) => {}
            None => self.buf.clear(),
        }
    }
}

/// SPP 通道：流 + 帧累积器 + 自动 ACK，供认证/推送共用。
pub struct SppChannel<'a> {
    pub stream: &'a mut bluer::rfcomm::Stream,
    pub acc: V2Accumulator,
}

impl<'a> SppChannel<'a> {
    pub fn new(stream: &'a mut bluer::rfcomm::Stream) -> Self {
        Self { stream, acc: V2Accumulator::new() }
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        self.stream
            .write_all(data)
            .await
            .map_err(|e| format!("SPP 写入失败: {e}"))
    }

    /// 读一块原始字节并累积。返回 Ok(true) 有新数据，Ok(false) 连接断开。
    pub async fn read_more(&mut self) -> Result<bool, String> {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 8192];
        let n = self.stream.read(&mut buf).await.map_err(|e| format!("SPP 读取失败: {e}"))?;
        if n == 0 {
            return Ok(false);
        }
        self.acc.feed(&buf[..n]);
        Ok(true)
    }

    /// 从累积器取走所有完整帧，Data 帧自动回 ACK。返回帧列表。
    pub async fn drain_ack(&mut self) -> Result<Vec<(u8, u8, Vec<u8>)>, String> {
        let frames = self.acc.drain();
        for (pt, seq, _) in &frames {
            if *pt == V2_PACKET_DATA {
                self.write(&build_ack_frame(*seq)).await?;
            }
        }
        Ok(frames)
    }
}


pub fn build_ack_frame(seq: u8) -> Vec<u8> {
    encode_v2_frame(V2_PACKET_ACK, seq, &[])
}

pub fn build_session_config(opcode: u8) -> Vec<u8> {
    // TLV 参数来自参考实现（协议笔记 4 节，POC golden 验证）
    let tlvs: &[(u8, &[u8])] = &[
        (1, &[0x01, 0x00, 0x00]), // KEY_VERSION 01.00.00
        (2, &[0x00, 0xfc]),       // MAX_PACKET_SIZE 0xFC00
        (3, &[0x20, 0x00]),       // TX_WIN 32
        (4, &[0x10, 0x27]),       // SEND_TIMEOUT 10000ms
    ];
    let mut payload = vec![opcode];
    for (key, value) in tlvs {
        payload.push(*key);
        payload.extend_from_slice(&(value.len() as u16).to_le_bytes());
        payload.extend_from_slice(value);
    }
    encode_v2_frame(V2_PACKET_SESSION_CONFIG, 0, &payload)
}

/// PROTOBUF 通道 Data 帧（握手阶段明文；认证后 encrypt=true）。
pub fn build_protobuf_frame(seq: u8, cmd: &[u8], encrypt: bool, key: &[u8]) -> Vec<u8> {
    let (opcode, body) = if encrypt {
        (OPCODE_ENCRYPTED, encrypt_v2(key, cmd))
    } else {
        (OPCODE_PLAINTEXT, cmd.to_vec())
    };
    let mut payload = vec![CHANNEL_PROTOBUF & 0x0F, opcode & 0xFF];
    payload.extend_from_slice(&body);
    encode_v2_frame(V2_PACKET_DATA, seq, &payload)
}

// ---------------------------------------------------------------------------
// protobuf 编解码（手写 varint）
// ---------------------------------------------------------------------------

pub fn varint_encode(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

pub fn field_varint(num: u64, value: u64) -> Vec<u8> {
    let mut out = varint_encode((num << 3) | 0);
    out.extend_from_slice(&varint_encode(value));
    out
}

pub fn field_bytes(num: u64, data: &[u8]) -> Vec<u8> {
    let mut out = varint_encode((num << 3) | 2);
    out.extend_from_slice(&varint_encode(data.len() as u64));
    out.extend_from_slice(data);
    out
}

pub fn field_fixed32(num: u64, value: f32) -> Vec<u8> {
    let mut out = varint_encode((num << 3) | 5);
    out.extend_from_slice(&value.to_le_bytes());
    out
}

/// 解析后的 protobuf 字段值。
#[derive(Debug, Clone, PartialEq)]
pub enum ProtoVal {
    Varint(u64),
    Bytes(Vec<u8>),
}

/// 扫描 protobuf 字段，返回 [(field_num, ProtoVal)]。
pub fn parse_proto_fields(data: &[u8]) -> Result<Vec<(u64, ProtoVal)>, String> {
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let (tag, p) = read_varint(data, pos)?;
        pos = p;
        let num = tag >> 3;
        let wt = (tag & 7) as u8;
        match wt {
            0 => {
                let (v, p) = read_varint(data, pos)?;
                pos = p;
                fields.push((num, ProtoVal::Varint(v)));
            }
            2 => {
                let (len, p) = read_varint(data, pos)?;
                pos = p;
                if pos + len as usize > data.len() {
                    return Err("length-delimited 越界".into());
                }
                let v = data[pos..pos + len as usize].to_vec();
                pos += len as usize;
                fields.push((num, ProtoVal::Bytes(v)));
            }
            other => return Err(format!("不支持的 wire type {other}")),
        }
    }
    Ok(fields)
}

fn read_varint(data: &[u8], mut pos: usize) -> Result<(u64, usize), String> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        if pos >= data.len() {
            return Err("varint 截断".into());
        }
        let b = data[pos];
        pos += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok((result, pos));
        }
        shift += 7;
        if shift > 63 {
            return Err("varint 过长".into());
        }
    }
}

// ---------------------------------------------------------------------------
// 认证算法（协议笔记 4.4/4.5 节，移植自 POC）
// ---------------------------------------------------------------------------

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC key 任意长度");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// 派生 64B 会话材料：(0-15)decKey (16-31)encKey (32-35)decNonce (36-39)encNonce。
pub fn derive_session(secret: &[u8; 16], phone_nonce: &[u8; 16], watch_nonce: &[u8; 16]) -> [u8; 64] {
    let mut k = Vec::with_capacity(32);
    k.extend_from_slice(phone_nonce);
    k.extend_from_slice(watch_nonce);
    let intermediate = hmac_sha256(&k, secret);
    let mut out = Vec::new();
    let mut tmp: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while out.len() < 64 {
        let mut msg = tmp.clone();
        msg.extend_from_slice(b"miwear-auth");
        msg.push(counter);
        tmp = hmac_sha256(&intermediate, &msg);
        out.extend_from_slice(&tmp);
        counter += 1;
    }
    let mut arr = [0u8; 64];
    arr.copy_from_slice(&out[..64]);
    arr
}

pub fn verify_watch_hmac(
    dec_key: &[u8],
    watch_nonce: &[u8],
    phone_nonce: &[u8],
    watch_hmac: &[u8],
) -> bool {
    let mut msg = Vec::with_capacity(watch_nonce.len() + phone_nonce.len());
    msg.extend_from_slice(watch_nonce);
    msg.extend_from_slice(phone_nonce);
    let expected = hmac_sha256(dec_key, &msg);
    // 恒定时间比较
    expected.len() == watch_hmac.len()
        && expected
            .iter()
            .zip(watch_hmac.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// HMAC-SHA256(key=encKey, msg=phoneNonce||watchNonce)，供 AuthStep3.encryptedNonces。
pub fn phone_ack(enc_key: &[u8], phone_nonce: &[u8], watch_nonce: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(phone_nonce.len() + watch_nonce.len());
    msg.extend_from_slice(phone_nonce);
    msg.extend_from_slice(watch_nonce);
    hmac_sha256(enc_key, &msg)
}

/// AES-128-CCM，nonce=(encNonce4, 4×0x00, counter u32LE)，macBits=32。输出 ct||tag(4B)。
pub fn encrypt_v1(key: &[u8], enc_nonce4: &[u8], counter: u32, payload: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(enc_nonce4);
    nonce[4..8].copy_from_slice(&[0u8; 4]);
    nonce[8..12].copy_from_slice(&counter.to_le_bytes());
    let cipher = <Aes128Ccm4 as ccm::aead::KeyInit>::new_from_slice(key).expect("AES-128 key 16 字节");
    cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: payload, aad: &[] })
        .expect("CCM 加密")
}

/// AES-128-CTR，key 即 IV（legacy quirk，协议笔记 4 节）。
pub fn encrypt_v2(key: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut cipher = Ctr128BE::<Aes128>::new(key.into(), key.into());
    let mut out = payload.to_vec();
    cipher.apply_keystream(&mut out);
    out
}

pub fn decrypt_v2(key: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    encrypt_v2(key, ciphertext) // CTR 对称
}

/// authkey：32 hex 字符 = 16 字节，支持 "0x" 前缀。非法返回 None。
pub fn parse_authkey(authkey_hex: &str) -> Option<[u8; 16]> {
    let mut s = authkey_hex.trim();
    if s.len() >= 2 && s[..2].eq_ignore_ascii_case("0x") {
        s = &s[2..];
    }
    if s.len() != AUTHKEY_LEN {
        return None;
    }
    let mut out = [0u8; 16];
    let mut ok = true;
    for i in 0..16 {
        match u8::from_str_radix(&s[i * 2..i * 2 + 2], 16) {
            Ok(b) => out[i] = b,
            Err(_) => {
                ok = false;
                break;
            }
        }
    }
    ok.then_some(out)
}

// ---------------------------------------------------------------------------
// WearPacket 编码（协议笔记 5 节，astrobox wear.proto）
// ---------------------------------------------------------------------------

/// WearPacket{type=1, id=2, payload_field=body}。payload_field 为字段号（WatchFace=6, Mass=24）。
fn encode_wear_packet(pkt_type: u8, pkt_id: u8, payload_field: u64, payload_body: &[u8]) -> Vec<u8> {
    let mut out = field_varint(1, pkt_type as u64);
    out.extend_from_slice(&field_varint(2, pkt_id as u64));
    out.extend_from_slice(&field_bytes(payload_field, payload_body));
    out
}

/// WatchFace PREPARE_INSTALL（prepare_info{id, size, version_code=65536}）。
pub fn encode_watchface_prepare(watchface_id: &str, size: u32) -> Vec<u8> {
    let mut info = field_bytes(1, watchface_id.as_bytes());
    info.extend_from_slice(&field_varint(2, size as u64));
    info.extend_from_slice(&field_varint(3, 65536));
    let wf = field_bytes(6, &info); // WatchFace payload（字段 6）
    encode_wear_packet(WEARPACKET_TYPE_WATCH_FACE, WP_ID_PREPARE_INSTALL_WATCH_FACE, 6, &wf)
}

/// Mass PREPARE（prepare_request{data_type=16, data_id=md5, data_length}）。
pub fn encode_mass_prepare(md5: &[u8], size: u32) -> Vec<u8> {
    let mut req = field_varint(1, MASS_DATA_TYPE as u64);
    req.extend_from_slice(&field_bytes(2, md5));
    req.extend_from_slice(&field_varint(3, size as u64));
    let mass = field_bytes(1, &req); // Mass payload（字段 24，非 7！）
    encode_wear_packet(WEARPACKET_TYPE_MASS, WP_ID_MASS_PREPARE, WEARPACKET_PAYLOAD_MASS as u64, &mass)
}

/// 解析收包 WearPacket 关键字段。
#[derive(Debug, Default, Clone)]
pub struct WearPacket {
    pub typ: Option<u8>,
    pub id: Option<u8>,
    pub prepare_status: Option<u8>,
    pub slice_length: Option<usize>,
    pub install_result_code: Option<u8>,
}

pub fn parse_wear_packet(data: &[u8]) -> Option<WearPacket> {
    let fields = parse_proto_fields(data).ok()?;
    let mut wp = WearPacket::default();
    for (num, val) in &fields {
        match (num, val) {
            (1, ProtoVal::Varint(v)) => wp.typ = Some(*v as u8),
            (2, ProtoVal::Varint(v)) => wp.id = Some(*v as u8),
            _ => {}
        }
    }
    // WatchFace payload（字段 6）
    for (num, val) in &fields {
        if *num == 6 {
            if let ProtoVal::Bytes(b) = val {
                let wf = parse_proto_fields(b).ok()?;
                for (wn, wv) in &wf {
                    match (wn, wv) {
                        (5, ProtoVal::Varint(v)) => wp.prepare_status = Some(*v as u8),
                        (7, ProtoVal::Bytes(ir)) => {
                            // install_result{id=1, code=2}
                            if let Ok(irf) = parse_proto_fields(ir) {
                                for (in_, iv) in &irf {
                                    if *in_ == 2 {
                                        if let ProtoVal::Varint(v) = iv {
                                            wp.install_result_code = Some(*v as u8);
                                        }
                                    }
                                }
                            }
                        }
                        (9, ProtoVal::Bytes(pr)) => {
                            // prepare_reply{...}，可能含 slice_length（字段 4 或 5）
                            if let Ok(prf) = parse_proto_fields(pr) {
                                for (pn_, pv) in &prf {
                                    if let ProtoVal::Varint(v) = pv {
                                        if *pn_ == 2 {
                                            wp.prepare_status = Some(*v as u8);
                                        }
                                        if *pn_ == 4 || *pn_ == 5 {
                                            wp.slice_length = Some(*v as usize);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    // Mass payload（字段 24 = WEARPACKET_PAYLOAD_MASS，非 type 22）
    for (num, val) in &fields {
        if *num == WEARPACKET_PAYLOAD_MASS as u64 {
            if let ProtoVal::Bytes(b) = val {
                let m = parse_proto_fields(b).ok()?;
                for (mn, mv) in &m {
                    if *mn == 2 {
                        if let ProtoVal::Bytes(pr) = mv {
                            if let Ok(prf) = parse_proto_fields(pr) {
                                for (pn_, pv) in &prf {
                                    if let ProtoVal::Varint(v) = pv {
                                        if *pn_ == 2 {
                                            wp.prepare_status = Some(*v as u8);
                                        }
                                        if *pn_ == 5 {
                                            wp.slice_length = Some(*v as usize);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Some(wp)
}

// ---------------------------------------------------------------------------
// Command 编解码（协议笔记 4.4 节：Command{type=1, subtype=2, auth=3}，字节兼容 WearPacket）
// ---------------------------------------------------------------------------

/// PhoneNonce{nonce=1}
pub fn encode_phone_nonce(nonce: &[u8]) -> Vec<u8> {
    field_bytes(1, nonce)
}

/// WatchNonce{nonce=1, hmac=2}
pub fn encode_watch_nonce(nonce: &[u8], hmac: &[u8]) -> Vec<u8> {
    let mut out = field_bytes(1, nonce);
    out.extend_from_slice(&field_bytes(2, hmac));
    out
}

/// AuthStep3{encryptedNonces=1, encryptedDeviceInfo=2}
pub fn encode_auth_step3(encrypted_nonces: &[u8], encrypted_device_info: &[u8]) -> Vec<u8> {
    let mut out = field_bytes(1, encrypted_nonces);
    out.extend_from_slice(&field_bytes(2, encrypted_device_info));
    out
}

/// AuthDeviceInfo{unknown1=1(uint32), phoneApiLevel=2(float), phoneName=3(string), unknown3=4(uint32), region=5(string)}
pub fn encode_auth_device_info(
    unknown1: u32,
    phone_api_level: f32,
    phone_name: &[u8],
    unknown3: u32,
    region: &[u8],
) -> Vec<u8> {
    let mut out = field_varint(1, unknown1 as u64);
    out.extend_from_slice(&field_fixed32(2, phone_api_level));
    out.extend_from_slice(&field_bytes(3, phone_name));
    out.extend_from_slice(&field_varint(4, unknown3 as u64));
    out.extend_from_slice(&field_bytes(5, region));
    out
}

/// Command{type=1, subtype=2, auth=3}
pub fn encode_command(type_: u8, subtype: Option<u8>, auth: Option<&[u8]>) -> Vec<u8> {
    let mut out = field_varint(1, type_ as u64);
    if let Some(st) = subtype {
        out.extend_from_slice(&field_varint(2, st as u64));
    }
    if let Some(a) = auth {
        out.extend_from_slice(&field_bytes(3, a));
    }
    out
}

/// Command{type=1, subtype=26(CMD_NONCE), Auth{phoneNonce=30}}
pub fn encode_command_phone_nonce(nonce: &[u8]) -> Vec<u8> {
    let auth = field_bytes(30, &encode_phone_nonce(nonce));
    encode_command(1, Some(26), Some(&auth))
}

/// Command{type=1, subtype=26, Auth{watchNonce=31}}（收包构造/测试用）
pub fn encode_command_watch_nonce(nonce: &[u8], hmac: &[u8]) -> Vec<u8> {
    let auth = field_bytes(31, &encode_watch_nonce(nonce, hmac));
    encode_command(1, Some(26), Some(&auth))
}

/// Command{type=1, subtype=27(CMD_AUTH), Auth{authStep3=32}}
pub fn encode_command_auth_step3(encrypted_nonces: &[u8], encrypted_device_info: &[u8]) -> Vec<u8> {
    let auth = field_bytes(32, &encode_auth_step3(encrypted_nonces, encrypted_device_info));
    encode_command(1, Some(27), Some(&auth))
}

/// 解析收包 Command：type / subtype / phone_nonce / watch_nonce / watch_hmac。
#[derive(Debug, Default, Clone)]
pub struct Command {
    pub typ: Option<u8>,
    pub subtype: Option<u8>,
    pub phone_nonce: Option<Vec<u8>>,
    pub watch_nonce: Option<Vec<u8>>,
    pub watch_hmac: Option<Vec<u8>>,
}

pub fn parse_command(data: &[u8]) -> Option<Command> {
    let fields = parse_proto_fields(data).ok()?;
    let mut cmd = Command::default();
    let mut auth: Option<Vec<u8>> = None;
    for (num, val) in &fields {
        match (num, val) {
            (1, ProtoVal::Varint(v)) => cmd.typ = Some(*v as u8),
            (2, ProtoVal::Varint(v)) => cmd.subtype = Some(*v as u8),
            (3, ProtoVal::Bytes(b)) => auth = Some(b.clone()),
            _ => {}
        }
    }
    let auth = auth?;
    let auth_fields = parse_proto_fields(&auth).ok()?;
    for (num, val) in &auth_fields {
        match (num, val) {
            (30, ProtoVal::Bytes(b)) => {
                // Auth.phoneNonce{nonce=1}
                if let Ok(pf) = parse_proto_fields(b) {
                    for (pn, pv) in &pf {
                        if *pn == 1 {
                            if let ProtoVal::Bytes(n) = pv {
                                cmd.phone_nonce = Some(n.clone());
                            }
                        }
                    }
                }
            }
            (31, ProtoVal::Bytes(b)) => {
                // Auth.watchNonce{nonce=1, hmac=2}
                if let Ok(wf) = parse_proto_fields(b) {
                    for (wn, wv) in &wf {
                        match (wn, wv) {
                            (1, ProtoVal::Bytes(n)) => cmd.watch_nonce = Some(n.clone()),
                            (2, ProtoVal::Bytes(h)) => cmd.watch_hmac = Some(h.clone()),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Some(cmd)
}

// ---------------------------------------------------------------------------
// 测试（POC golden 字节交叉验证）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // golden 值（POC self_test 验证过）
    const G_SESSION_PAYLOAD: &str = "0101030001000002020000fc03020020000402001027";
    const G_PHONE_NONCE_CMD: &str = "0801101a1a15f201120a1000000000000000000000000000000000";
    const G_WATCH_NONCE_CMD: &str = "0801101a1a37fa01340a100000000000000000000000000000000012201111111111111111111111111111111111111111111111111111111111111111";
    const G_AUTH_STEP3_CMD: &str = "0801101b1a378202340a100000000000000000000000000000000012201111111111111111111111111111111111111111111111111111111111111111";
    const G_DEVICE_INFO: &str = "080015000008421a03504f4320e0012a02434e";
    const G_CCM_OUT: &str = "eed3267521be8d33e40d8b9a42022d3cb1f7d769";
    const G_CTR_OUT: &str = "14f42f3141958835925097f1ac005c504d271f9022197c014f5a8dee8f83adaf";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn crc16_arc_standard_vector() {
        assert_eq!(crc16_arc(b"123456789"), 0xBB3D);
    }

    #[test]
    fn crc16_arc_equals_reference_impl() {
        // POC 的 crc_kotlin_form（左移 + 最终位反转式）
        fn crc_kotlin_form(data: &[u8]) -> u16 {
            let mut crc: u32 = 0;
            for &byte in data {
                for j in 0..8u32 {
                    crc = (crc << 1) & 0xFFFFFFFF;
                    let bit = ((crc >> 16) & 1) ^ ((byte as u32 >> j) & 1);
                    if bit == 1 {
                        crc = (crc ^ 0x8005) & 0xFFFFFFFF;
                    }
                }
            }
            let rev = format!("{crc:032b}").chars().rev().collect::<String>();
            (u32::from_str_radix(&rev, 2).unwrap() >> 16) as u16
        }
        for data in [&b""[..], b"\x00", b"\xa5\xa5", &(0..=255u8).collect::<Vec<_>>(), b"hello world"] {
            assert_eq!(crc16_arc(data), crc_kotlin_form(data));
        }
    }

    #[test]
    fn v2_frame_roundtrip() {
        let payload = hex(G_SESSION_PAYLOAD);
        let frame = encode_v2_frame(V2_PACKET_SESSION_CONFIG, 0, &payload);
        assert_eq!(frame, {
            let mut v = hex("a5a5020016001d4d");
            v.extend_from_slice(&payload);
            v
        });
        let parsed = parse_v2_frame(&frame).unwrap().unwrap();
        assert_eq!(parsed, (V2_PACKET_SESSION_CONFIG, 0, payload));
    }

    #[test]
    fn v2_frame_incomplete_and_corrupt() {
        let payload = hex(G_SESSION_PAYLOAD);
        let frame = encode_v2_frame(V2_PACKET_SESSION_CONFIG, 0, &payload);
        assert!(parse_v2_frame(&frame[..7]).unwrap().is_none());
        let mut bad = frame.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert!(parse_v2_frame(&bad).is_err());
    }

    #[test]
    fn accumulator_reassembles_frames() {
        let f1 = encode_v2_frame(V2_PACKET_SESSION_CONFIG, 0, &hex(G_SESSION_PAYLOAD));
        let f2 = encode_v2_frame(V2_PACKET_DATA, 3, &[0x01, 0x01, 0x08, 0x01]);
        let f3 = encode_v2_frame(V2_PACKET_ACK, 9, &[]);
        let stream: Vec<u8> = [f1.as_slice(), &f2, &f3].concat();
        // 1 字节切块
        let mut acc = V2Accumulator::new();
        let mut out = Vec::new();
        for b in &stream {
            acc.feed(&[*b]);
            out.extend(acc.drain());
        }
        assert_eq!(out.len(), 3);
        assert!(acc.buf.is_empty());
        // 非法前缀重同步
        let mut junk = vec![0xde, 0xad, 0xbe, 0xef];
        junk.extend_from_slice(&stream);
        let out = V2Accumulator::new().feed_drain(&junk);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].0, V2_PACKET_SESSION_CONFIG);
    }

    #[test]
    fn protobuf_golden_vectors() {
        let nonce16 = [0u8; 16];
        let mac32 = [0x11u8; 32];

        // PhoneNonce 命令
        let cmd = encode_command_phone_nonce(&nonce16);
        assert_eq!(cmd, hex(G_PHONE_NONCE_CMD));

        // WatchNonce 命令
        let wn_cmd = encode_command_watch_nonce(&nonce16, &mac32);
        assert_eq!(wn_cmd, hex(G_WATCH_NONCE_CMD));

        // AuthStep3 命令
        let s3 = encode_command_auth_step3(&nonce16, &mac32);
        assert_eq!(s3, hex(G_AUTH_STEP3_CMD));

        // AuthDeviceInfo
        let info = encode_auth_device_info(0, 34.0f32, b"POC", 224, b"CN");
        assert_eq!(info, hex(G_DEVICE_INFO));
    }

    #[test]
    fn derive_session_is_deterministic_and_64b() {
        let secret = hex("00112233445566778899aabbccddeeff");
        let pn = hex("000102030405060708090a0b0c0d0e0f");
        let wn = hex("101112131415161718191a1b1c1d1e1f");
        let mut s1 = [0u8; 16];
        let mut p1 = [0u8; 16];
        let mut w1 = [0u8; 16];
        s1.copy_from_slice(&secret);
        p1.copy_from_slice(&pn);
        w1.copy_from_slice(&wn);
        let d1 = derive_session(&s1, &p1, &w1);
        let d2 = derive_session(&s1, &p1, &w1);
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64);

        // verify_watch_hmac 正反例
        let dec_key = &d1[0..16];
        let mut msg = wn.clone();
        msg.extend_from_slice(&pn);
        let wh = hmac_sha256(dec_key, &msg);
        assert!(verify_watch_hmac(dec_key, &wn, &pn, &wh));
        let mut bad = wh.clone();
        bad[0] ^= 1;
        assert!(!verify_watch_hmac(dec_key, &wn, &pn, &bad));

        // phone_ack
        let enc_key = &d1[16..32];
        let mut msg2 = pn.clone();
        msg2.extend_from_slice(&wn);
        assert_eq!(phone_ack(enc_key, &pn, &wn), hmac_sha256(enc_key, &msg2));
    }

    #[test]
    fn aes_v1_ccm_golden() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let plain = hex("6bc1bee22e409f96e93d7e117393172a");
        let enc_nonce4 = hex("deadbeef");
        let out = encrypt_v1(&key, &enc_nonce4, 7, &plain);
        assert_eq!(out, hex(G_CCM_OUT));
    }

    #[test]
    fn aes_v2_ctr_golden_and_roundtrip() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let plain = {
            let p = hex("6bc1bee22e409f96e93d7e117393172a");
            [p.as_slice(), &p].concat()
        };
        assert_eq!(encrypt_v2(&key, &plain), hex(G_CTR_OUT));
        let empty: &[u8] = &[];
        assert_eq!(decrypt_v2(&key, &encrypt_v2(&key, empty)), empty);
    }

    #[test]
    fn parse_authkey_accepts_0x_prefix() {
        let expect = hex("abababababababababababababababab");
        assert_eq!(parse_authkey("abababababababababababababababab"), Some(expect.clone().try_into().unwrap()));
        assert_eq!(parse_authkey("0xabababababababababababababababab"), Some(expect.clone().try_into().unwrap()));
        assert_eq!(parse_authkey("0Xabababababababababababababababab"), Some(expect.clone().try_into().unwrap()));
        assert_eq!(parse_authkey("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"), None);
        assert_eq!(parse_authkey("ab"), None);
    }

    #[test]
    fn wearpacket_encode_parse() {
        // WatchFace prepare
        let pkt = encode_watchface_prepare("167210067", 2492348);
        assert_eq!(pkt[0..2], [0x08, 0x04]); // type=4
        assert_eq!(pkt[2..4], [0x10, 0x04]); // id=4
        let wp = parse_wear_packet(&pkt).unwrap();
        assert_eq!(wp.typ, Some(WEARPACKET_TYPE_WATCH_FACE));
        assert_eq!(wp.id, Some(WP_ID_PREPARE_INSTALL_WATCH_FACE));

        // Mass prepare：type 字节应为 08 16 = 22（type=MASS），payload 字段 = 24
        let mass = encode_mass_prepare(&[0x11; 16], 100);
        assert_eq!(mass[0..2], [0x08, 0x16]); // type=MASS(22)
        assert_eq!(mass[2..4], [0x10, 0x00]); // id=PREPARE(0)
        // payload 字段应为 24（oneof）
        let mass_payload_field = mass[4] >> 3;
        assert_eq!(mass_payload_field, 24);
        let wp = parse_wear_packet(&mass).unwrap();
        assert_eq!(wp.typ, Some(WEARPACKET_TYPE_MASS));
        assert_eq!(wp.id, Some(WP_ID_MASS_PREPARE));
    }
}
