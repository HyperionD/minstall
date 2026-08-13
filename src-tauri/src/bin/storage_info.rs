//! 只读查询：连接手环 → 认证 → GET_STORAGE_INFO，打印存储使用。
//! 用法: cargo run --bin storage_info -- <addr> <authkey>
use std::time::Instant;

use minstall_lib::ble::connection::Manager;
use minstall_lib::protocol::auth;
use minstall_lib::protocol::consts::*;
use minstall_lib::protocol::encoding::*;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: storage_info <addr> <authkey>");
        std::process::exit(2);
    }
    let mut mgr = Manager::new();
    let t0 = Instant::now();
    eprintln!("[storage] 连接 {} ...", args[1]);
    mgr.connect(&args[1]).await.expect("连接失败");
    let session = auth::authenticate(&mut mgr, &args[2], None).await.expect("认证失败");
    eprintln!("[storage] ✅ 认证成功 ({:?})", t0.elapsed());

    let stream = mgr.stream_mut().expect("stream");
    let mut ch = SppChannel::new(stream);

    // GET_STORAGE_INFO: WearPacket{type=SYSTEM(2), id=62}，无 payload
    let pkt = {
        let mut out = field_varint(1, WEARPACKET_TYPE_SYSTEM as u64);
        out.extend_from_slice(&field_varint(2, WP_ID_GET_STORAGE_INFO as u64));
        out
    };
    let frame = build_protobuf_frame(session.seq, &pkt, true, &session.enc_key);
    ch.write(&frame).await.expect("写入失败");
    eprintln!("[storage] → GET_STORAGE_INFO");

    // 等待响应（加密通道），打印所有 WearPacket
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15);
    loop {
        if tokio::time::Instant::now() >= deadline {
            eprintln!("[storage] ❌ 等待响应超时");
            break;
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), ch.read_more()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => { eprintln!("[storage] SPP 断开"); break; }
            Ok(Err(e)) => { eprintln!("[storage] 读取错误: {e}"); break; }
            Err(_) => continue,
        }
        for (pt, _seq, payload) in ch.drain_ack().await.expect("drain") {
            if pt != V2_PACKET_DATA {
                continue;
            }
            if let Some(body) = protobuf_body_pub(&payload, &session) {
                eprintln!("[storage] 明文({}B): {}", body.len(), hex(&body));
                parse_storage(&body);
            }
        }
    }
}

fn parse_storage(body: &[u8]) {
    let fields = match parse_proto_fields(body) {
        Ok(f) => f,
        Err(e) => { eprintln!("[storage] 解析失败: {e}"); return; }
    };
    for (num, val) in &fields {
        if *num == 4 {
            // System payload
            if let ProtoVal::Bytes(b) = val {
                if let Ok(sys) = parse_proto_fields(b) {
                    for (sn, sv) in &sys {
                        if *sn == 44 {
                            // storage_info{used=1, total=2}
                            if let ProtoVal::Bytes(si) = sv {
                                if let Ok(info) = parse_proto_fields(si) {
                                    let mut used: u64 = 0;
                                    let mut total: u64 = 0;
                                    for (in_, iv) in &info {
                                        match (in_, iv) {
                                            (1, ProtoVal::Varint(v)) => used = *v,
                                            (2, ProtoVal::Varint(v)) => total = *v,
                                            _ => {}
                                        }
                                    }
                                    eprintln!("[storage] ★ used={} bytes ({:.2} MB)  total={} bytes ({:.2} MB)", used, used as f64 / 1048576.0, total, total as f64 / 1048576.0);
                                    eprintln!("[storage] ★ 可用 = {:.2} MB", (total - used) as f64 / 1048576.0);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
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
