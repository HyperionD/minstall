// 最小 bluer SPP 连接测试：仅 register_profile + connect_profile
use bluer::rfcomm::{Profile, Role};
use bluer::{Adapter, Address, Session, Uuid};
use futures::StreamExt;

const SPP_UUID: u128 = 0x00001101_0000_1000_8000_00805f9b34fb;

#[tokio::main]
async fn main() {
    let addr: Address = "2C:0D:CF:73:D9:95".parse().unwrap();
    let session = Session::new().await.unwrap();
    let adapter = session.default_adapter().await.unwrap();
    adapter.set_powered(true).await.unwrap();
    let device = adapter.device(addr).unwrap();

    eprintln!("[t] paired={:?} connected={:?}", device.is_paired().await.unwrap(), device.is_connected().await.unwrap());

    // 注册 Profile（同 POC opts）
    let profile = Profile {
        uuid: Uuid::from_u128(SPP_UUID),
        name: Some("minstall-spp".to_string()),
        role: Some(Role::Client),
        require_authentication: Some(false),
        require_authorization: Some(false),
        ..Default::default()
    };
    let mut handle = session.register_profile(profile).await.unwrap();
    eprintln!("[t] Profile 已注册");

    // ConnectProfile（与 POC 相同）
    match device.connect_profile(&Uuid::from_u128(SPP_UUID)).await {
        Ok(()) => eprintln!("[t] ConnectProfile OK"),
        Err(e) => { eprintln!("[t] ConnectProfile 失败: {e}"); return; }
    }

    // 等 ConnectRequest
    match tokio::time::timeout(std::time::Duration::from_secs(10), handle.next()).await {
        Ok(Some(req)) => {
            eprintln!("[t] 收到 ConnectRequest from {}", req.device());
            let _stream = req.accept().unwrap();
            eprintln!("[t] accept OK, SPP 连接建立");
        }
        Ok(None) => eprintln!("[t] 流结束"),
        Err(_) => eprintln!("[t] 等待 ConnectRequest 超时"),
    }
}
