//! SPP 连接管理：BLE 配对（DisplayYesNo agent）→ 建链 → BlueZ Profile 连接（RFCOMM ch5）。
//!
//! 流程依据 docs/protocol-notes.md 4.3/4.5 节（真机验证）：
//! 1. 注册 DisplayYesNo agent（手环配对模式需要确认，NoInputNoOutput 会失败）
//! 2. 设备未配对则 pair()（agent 自动接受确认）
//! 3. connect() 建 ACL + SDP
//! 4. 注册 SPP Profile（client）→ Device.ConnectProfile → ConnectRequest 流拿到 fd → Stream
//!
//! 注意：必须走 BlueZ Profile 机制（POC dbus-fast 的等价路径，AstroBox 同款），
//! 直接内核 socket 直连（rfcomm::Stream::connect）手环不响应（无 Profile 握手）。

use bluer::agent::{Agent, AgentHandle, RequestConfirmation};
use bluer::rfcomm::{Profile, ProfileHandle, Role};
use bluer::{Adapter, Address, Session, Uuid};
use futures::StreamExt;

use super::errors::BleError;
use crate::protocol::auth::Session as AuthSession;

pub struct Manager {
    stream: Option<bluer::rfcomm::Stream>,
    session: Option<AuthSession>,
    /// 会话内下一个可用的发送 seq（认证后从 2 起；跨 command 连续，避免与已发帧冲突）
    seq: u8,
    bluer_session: Option<Session>,
    agent: Option<AgentHandle>,
    adapter: Option<Adapter>,
    profile: Option<ProfileHandle>,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            stream: None,
            session: None,
            seq: 0,
            bluer_session: None,
            agent: None,
            adapter: None,
            profile: None,
        }
    }

    /// 当前可用 seq（未认证时为 0）。
    pub fn seq(&self) -> u8 {
        self.seq
    }

    /// 设置/推进 seq（发送帧后调用，wrapping）。
    pub fn advance_seq(&mut self, next: u8) {
        self.seq = next;
    }

    /// 建立 SPP 连接：配对（如需）→ 建链 → BlueZ Profile 连接。
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

        // 4) 先注册 SPP Profile（POC 顺序：RegisterProfile → Connect → ConnectProfile）
        //    POC 等价 dbus 路径：ProfileManager1.RegisterProfile + Device1.ConnectProfile
        if self.profile.is_none() {
            let profile = Profile {
                uuid: Uuid::from_u128(SPP_UUID_U128),
                name: Some("minstall-spp".to_string()),
                role: Some(Role::Client),
                // 不指定 channel：客户端由 SDP 自动发现远程通道（同 POC RegisterProfile 行为）
                require_authentication: Some(false),
                require_authorization: Some(false),
                // 不设置 auto_connect（同 POC opts：仅 Role/RequireAuthentication/RequireAuthorization）
                ..Default::default()
            };
            let handle = bluer_session
                .register_profile(profile)
                .await
                .map_err(|e| BleError::ConnectFailed(format!("注册 SPP Profile 失败: {e}")))?;
            self.profile = Some(handle);
        }

        // 5) 建链（ACL + SDP；配对后服务才可解析），等待链路建立（最长 10s）
        if !device.is_connected().await.unwrap_or(false) {
            match device.connect().await {
                Ok(()) => eprintln!("[minstall] Device.Connect() OK"),
                Err(e) => eprintln!("[minstall] Device.Connect() 失败: {e}"),
            }
        } else {
            eprintln!("[minstall] 设备已连接，跳过 Connect()");
        }
        for _ in 0..20 {
            if device.is_connected().await.unwrap_or(false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        // POC 时序：Connect() 后 sleep(2) 再 ConnectProfile（等 BR/EDR 连接收敛，避免 br-connection-busy）
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        // 6) ConnectProfile：只试 1 次（真机验证：第一次调用即触发连接，返回的错误
        //    是误导性的——ConnectRequest 会到达。重试（br-connection-create-socket）
        //    反而可能干扰已建立的 RFCOMM 连接，导致后续无响应）。
        match device.connect_profile(&Uuid::from_u128(SPP_UUID_U128)).await {
            Ok(()) => {}
            Err(e) => eprintln!("[minstall] ConnectProfile 返回错误（忽略，等待 ConnectRequest）: {e}"),
        }

        // 6) 等 ConnectRequest（fd）→ accept 为 Stream（需要可变借用 profile）
        let mut req_stream = self.profile.take().ok_or_else(|| BleError::ConnectFailed("SPP Profile 句柄丢失".into()))?;
        let request = tokio::time::timeout(std::time::Duration::from_secs(10), req_stream.next())
            .await
            .map_err(|_| BleError::ConnectFailed("等待 SPP 连接建立超时".into()))?
            .ok_or_else(|| BleError::ConnectFailed("SPP Profile 连接流已结束".into()))?;
        let stream = request
            .accept()
            .map_err(|e| BleError::ConnectFailed(format!("accept SPP 连接失败: {e}")))?;
        self.stream = Some(stream);
        Ok(())
    }

    /// 返回底层流可变引用（供协议层读写帧）。
    pub fn stream_mut(&mut self) -> Result<&mut bluer::rfcomm::Stream, BleError> {
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

/// 标准 Serial Port Profile UUID（协议笔记 4.5 节）
const SPP_UUID_U128: u128 = 0x00001101_0000_1000_8000_00805f9b34fb;
