//! 临时 e2e 验证程序（Task 16 真机验证用，验证后删除）。
//! 用法：
//!   cargo run --bin e2e -- scan
//!   cargo run --bin e2e -- full <addr> <authkey> <bin>
//!   cargo run --bin e2e -- badkey <addr> <wrong_authkey>
//!   cargo run --bin e2e -- badfile <bin>

use std::time::Instant;

use minstall_lib::ble::connection::Manager;
use minstall_lib::ble::scanner;
use minstall_lib::protocol::{auth, watchface};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法见文件头注释");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "scan" => do_scan().await,
        "full" => {
            if args.len() < 5 {
                eprintln!("full <addr> <authkey> <bin>");
                std::process::exit(2);
            }
            do_full(&args[2], &args[3], &args[4]).await;
        }
        "badkey" => {
            if args.len() < 4 {
                eprintln!("badkey <addr> <wrong_authkey>");
                std::process::exit(2);
            }
            do_badkey(&args[2], &args[3]).await;
        }
        "badfile" => {
            if args.len() < 3 {
                eprintln!("badfile <bin>");
                std::process::exit(2);
            }
            do_badfile(&args[2]).await;
        }
        other => {
            eprintln!("未知命令: {other}");
            std::process::exit(2);
        }
    }
}

async fn do_scan() {
    eprintln!("[e2e] 扫描 10s ...");
    match scanner::scan(10).await {
        Ok(devices) => {
            eprintln!("[e2e] 发现 {} 个相关设备:", devices.len());
            for d in &devices {
                eprintln!("  {}  {}  rssi={}", d.name, d.address, d.rssi);
            }
            if devices.is_empty() {
                eprintln!("[e2e] ❌ 未发现设备");
            } else {
                eprintln!("[e2e] ✅ 扫描发现设备");
            }
        }
        Err(e) => eprintln!("[e2e] ❌ 扫描失败: {e}"),
    }
}

async fn do_full(addr: &str, authkey: &str, bin_path: &str) {
    let mut mgr = Manager::new();
    let t0 = Instant::now();
    eprintln!("[e2e] 连接 {addr} ...");
    match mgr.connect(addr).await {
        Ok(()) => eprintln!("[e2e] ✅ 连接成功 ({:?})", t0.elapsed()),
        Err(e) => {
            eprintln!("[e2e] ❌ 连接失败: {e}");
            std::process::exit(1);
        }
    }
    eprintln!("[e2e] 认证 ...");
    match auth::authenticate(&mut mgr, authkey, None).await {
        Ok(session) => {
            eprintln!("[e2e] ✅ 认证成功 (seq={}, {:?})", session.seq, t0.elapsed());
            mgr.set_session(session);
        }
        Err(e) => {
            eprintln!("[e2e] ❌ 认证失败: {e}");
            std::process::exit(1);
        }
    }
    eprintln!("[e2e] 安装 {bin_path} ...");
    let session = match mgr.session() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[e2e] ❌ 取会话失败: {e}");
            std::process::exit(1);
        }
    };
    let t1 = Instant::now();
    match watchface::push(&mut mgr, &session, bin_path, |sent, total| {
        eprintln!("[e2e] 进度: {sent} / {total} 字节");
    })
    .await
    {
        Ok(()) => eprintln!("[e2e] ✅ 安装成功 ({:?})", t1.elapsed()),
        Err(e) => eprintln!("[e2e] ❌ 安装失败: {e}"),
    }
}

async fn do_badkey(addr: &str, wrong_key: &str) {
    let mut mgr = Manager::new();
    eprintln!("[e2e] 连接 {addr} ...");
    match mgr.connect(addr).await {
        Ok(()) => eprintln!("[e2e] ✅ 连接成功"),
        Err(e) => {
            eprintln!("[e2e] ❌ 连接失败: {e}");
            std::process::exit(1);
        }
    }
    eprintln!("[e2e] 用错误 authkey 认证（应失败）...");
    match auth::authenticate(&mut mgr, wrong_key, None).await {
        Ok(_) => eprintln!("[e2e] ❌ 意外成功（错误 authkey 应失败）"),
        Err(e) => eprintln!("[e2e] ✅ 认证失败提示符合预期: {e}"),
    }
}

async fn do_badfile(bin_path: &str) {
    // 不连接，直接验证文件前置校验（parse_watchface_id）
    let data = std::fs::read(bin_path).expect("读取文件失败");
    eprintln!("[e2e] 校验文件: {bin_path} ({} 字节)", data.len());
    match watchface::parse_watchface_id(&data) {
        Ok(id) => eprintln!("[e2e] ⚠️ 该文件解析出 id={id}（不是损坏文件）"),
        Err(e) => eprintln!("[e2e] ✅ 文件校验拦截符合预期: {e}"),
    }
}
