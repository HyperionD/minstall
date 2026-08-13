//! 上传测速工具（Task 16 后续）：验证不同 MASS_BATCH 下的上传速度与安装结果。
//! 用法:
//!   MINSTALL_MASS_BATCH=2 cargo run --bin speedtest -- <addr> <authkey> <bin>
//!   MINSTALL_MASS_BATCH=8 cargo run --bin speedtest -- <addr> <authkey> <bin>
use std::time::Instant;

use minstall_lib::ble::connection::Manager;
use minstall_lib::protocol::{auth, watchface};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: speedtest <addr> <authkey> <bin>  （BATCH 由 MINSTALL_MASS_BATCH 环境变量控制，默认 2）");
        std::process::exit(2);
    }
    let addr = &args[1];
    let authkey = &args[2];
    let bin_path = &args[3];
    let batch = std::env::var("MINSTALL_MASS_BATCH").unwrap_or_else(|_| "2".into());
    eprintln!("[speedtest] BATCH={batch}（MINSTALL_MASS_BATCH）");

    let mut mgr = Manager::new();
    let t0 = Instant::now();
    eprintln!("[speedtest] 连接 {addr} ...");
    mgr.connect(addr).await.expect("连接失败");
    eprintln!("[speedtest] ✅ 连接成功 ({:?})", t0.elapsed());

    let session = auth::authenticate(mgr.stream_mut().expect("连接"), authkey, None).await.expect("认证失败");
    eprintln!("[speedtest] ✅ 认证成功 ({:?})", t0.elapsed());
    mgr.set_session(session);

    let session = mgr.session().expect("取会话失败");
    let t1 = Instant::now();
    let mut seq = session.seq;
    match watchface::push(mgr.stream_mut().expect("连接"), &session, bin_path, &mut seq, |sent, total| {
        eprintln!("[speedtest] 进度: {sent} / {total} 字节");
    })
    .await
    {
        Ok(()) => eprintln!("[speedtest] ✅ 安装成功 ({:?})", t1.elapsed()),
        Err(e) => eprintln!("[speedtest] ❌ 安装失败: {e}"),
    }
}
