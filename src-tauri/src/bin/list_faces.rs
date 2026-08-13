//! 只读查询：连接手环 → 认证 → GET_INSTALLED_LIST，打印手环当前表盘列表（含 data_id/md5）。
//! 用法: cargo run --bin list_faces -- <addr> <authkey>
use std::time::Instant;

use minstall_lib::ble::connection::Manager;
use minstall_lib::protocol::auth;
use minstall_lib::protocol::consts::*;
use minstall_lib::protocol::encoding::*;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: list_faces <addr> <authkey>");
        std::process::exit(2);
    }
    let mut mgr = Manager::new();
    let t0 = Instant::now();
    eprintln!("[list] 连接 {} ...", args[1]);
    mgr.connect(&args[1]).await.expect("连接失败");
    eprintln!("[list] ✅ 连接成功 ({:?})", t0.elapsed());

    let session = auth::authenticate(&mut mgr, &args[2], None).await.expect("认证失败");
    eprintln!("[list] ✅ 认证成功 ({:?})", t0.elapsed());

    let stream = mgr.stream_mut().expect("stream");
    let mut ch = SppChannel::new(stream);

    // GET_INSTALLED_LIST: WearPacket{type=WATCH_FACE(4), id=0}
    let pkt = encode_wear_packet_public(WEARPACKET_TYPE_WATCH_FACE, WP_ID_GET_INSTALLED_LIST, 6, &[]);
    let frame = build_protobuf_frame(session.seq, &pkt, true, &session.enc_key);
    ch.write(&frame).await.expect("写入失败");
    eprintln!("[list] → GET_INSTALLED_LIST");

    // 等待响应（加密通道），打印所有收到的 WearPacket 完整字节
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15);
    loop {
        if tokio::time::Instant::now() >= deadline {
            eprintln!("[list] ❌ 等待响应超时");
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                eprintln!("[list] SPP 断开");
                break;
            }
            Ok(Err(e)) => {
                eprintln!("[list] 读取错误: {e}");
                break;
            }
            Err(_) => continue,
        }
        for (pt, seq, payload) in ch.drain_ack().await.expect("drain") {
            eprintln!("[list] V2帧 pt={pt} seq={seq} payload({}B)", payload.len());
            if pt != V2_PACKET_DATA {
                continue;
            }
            let body = protobuf_body_pub(&payload, &session);
            if let Some(body) = body {
                eprintln!("[list]   明文({}B): {}", body.len(), hex(&body));
                if let Some(wp) = parse_wear_packet(&body) {
                    eprintln!("[list]   WearPacket type={:?} id={:?}", wp.typ, wp.id);
                }
            }
        }
    }
}

pub fn encode_wear_packet_public(pkt_type: u8, pkt_id: u8, payload_field: u64, payload_body: &[u8]) -> Vec<u8> {
    let mut out = field_varint(1, pkt_type as u64);
    out.extend_from_slice(&field_varint(2, pkt_id as u64));
    if payload_field != 0 {
        out.extend_from_slice(&field_bytes(payload_field, payload_body));
    }
    out
}

pub fn protobuf_body_pub(payload: &[u8], session: &auth::Session) -> Option<Vec<u8>> {
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

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}
