//! SPP 连接管理：BLE 配对（DisplayYesNo agent）→ 建链 → RFCOMM ch5 流。
//!
//! 流程依据 docs/protocol-notes.md 4.5 节（真机验证）：
//! 1. 注册 DisplayYesNo agent（手环配对模式需要确认，NoInputNoOutput 会失败）
//! 2. 设备未配对则 pair()（agent 自动接受确认）
//! 3. connect() 建 ACL + SDP
//! 4. rfcomm::Stream::connect(RFCOMM_CHANNEL) 建立 SPP 流
//!
//! 由 Tauri command 层以 Arc<Mutex<Manager>> 共享（见 commands.rs）。

use bluer::agent::{Agent, AgentHandle, RequestConfirmation};
use bluer::rfcomm::{SocketAddr as RfcommSocketAddr, Stream};
use bluer::{Adapter, Address, Device, Session};

use super::errors::BleError;
use crate::protocol::auth::Session as AuthSession;
use crate::protocol::consts::RFCOMM_CHANNEL;

pub struct Manager {
    stream: Option<Stream>,
    session: Option<AuthSession>,
    bluer_session: Option<Session>,
    agent: Option<AgentHandle>,
    adapter: Option<Adapter>,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            stream: None,
            session: None,
            bluer_session: None,
            agent: None,
            adapter: None,
        }
    }

    /// 建立 SPP 连接：配对（如需）→ 建链 → RFCOMM ch5。
    pub async fn connect(&mut self, address: &str) -> Result<(), BleError> {
        let addr: Address = address
            .parse()
            .map_err(|_| BleError::ConnectFailed(format!("无效蓝牙地址: {address}")))?;

        // 1) BlueZ 会话 + 默认适配器（复用已持有句柄，保证 agent/device 引用存活）
        let bluer_session = match &self.bluer_session {
            Some(s) => s.clone(),
            None => {
                let s = Session::new().await.map_err(|_| BleError::Adapter)?;
                self.bluer_session = Some(s.clone());
                s
            }
        };
        let adapter = match &self.adapter {
            Some(a) => a.clone(),
            None => {
                let a = bluer_session
                    .default_adapter()
                    .await
                    .map_err(|_| BleError::Adapter)?;
                a.set_powered(true).await.map_err(|_| BleError::Adapter)?;
                self.adapter = Some(a.clone());
                a
            }
        };

        // 2) 注册 DisplayYesNo agent（真机验证：手环配对需确认）
        if self.agent.is_none() {
            let mut agent = Agent::default();
            agent.request_confirmation = Some(Box::new(|_: RequestConfirmation| {
                Box::pin(async { Ok(()) }) // 自动接受确认
            }));
            let handle = bluer_session
                .register_agent(agent)
                .await
                .map_err(|e| BleError::ConnectFailed(format!("注册配对 agent 失败: {e}")))?;
            self.agent = Some(handle);
        }

        // 3) 设备配对（未配对时；失败不阻塞——可能已配对或 agent 已处理）
        let device = adapter.device(addr).map_err(|e| BleError::ConnectFailed(e.to_string()))?;
        if !device.is_paired().await.unwrap_or(false) {
            eprintln!("[minstall] 设备未配对，发起配对（请在弹窗确认）...");
            let _ = device.pair().await;
        }

        // 4) 建链（ACL + SDP；配对后服务才可解析），等待链路建立（最长 10s）
        if !device.is_connected().await.unwrap_or(false) {
            let _ = device.connect().await;
        }
        for _ in 0..20 {
            if device.is_connected().await.unwrap_or(false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        // 5) RFCOMM 连接（V2 协议走 SPP，见协议笔记 4.3/4.5 节）
        let stream = Stream::connect(RfcommSocketAddr::new(addr, RFCOMM_CHANNEL))
            .await
            .map_err(|e| BleError::ConnectFailed(format!("SPP 连接失败: {e}")))?;
        self.stream = Some(stream);
        Ok(())
    }

    /// 返回底层流可变引用（供协议层读写帧）。
    pub fn stream_mut(&mut self) -> Result<&mut Stream, BleError> {
        self.stream
            .as_mut()
            .ok_or_else(|| BleError::ConnectFailed("未连接".into()))
    }

    /// 保存认证会话（供安装使用）。
    pub fn set_session(&mut self, session: AuthSession) {
        self.session = Some(session);
    }

    /// 取认证会话；未认证返回错误。
    pub fn session(&self) -> Result<AuthSession, BleError> {
        self.session
            .clone()
            .ok_or_else(|| BleError::AuthFailed("尚未认证（请先连接并输入 authkey）".into()))
    }

    /// 关闭连接并释放句柄。
    pub async fn disconnect(&mut self) {
        self.stream.take(); // drop 时自动 shutdown
        self.session = None;
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}
