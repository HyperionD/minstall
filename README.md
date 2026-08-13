# minstall

小米手环 10 Pro 表盘直装工具（蓝牙直连安装 .bin/.face 表盘，不经过官方 App）。

- 阶段 1：POC 协议验证（`pocs/`，Python + dbus-fast）—— **已完成（2026-08-12）**
- 阶段 2：Tauri 跨平台 GUI（`src-tauri/` + `src/`，Rust + React + TypeScript）—— **已完成真机验证（2026-08-13，Task 16 全项通过）**

## 文档

- [`docs/research-summary.md`](docs/research-summary.md) —— 研究历程与结论总结（人读）
- [`docs/protocol-notes.md`](docs/protocol-notes.md) —— 协议技术细节（真机验证数据）

## 技术路线（真机验证结论）

- Band 10 Pro 的 V2 协议走**经典蓝牙 SPP 通道**（RFCOMM ch5），非 BLE GATT（BLE 5e/5f 写帧无响应）
- 认证 + 表盘安装完整跑通（V1 Hello → authkey 认证 → WearPacket 协议 → MASS 分片上传）
- 表盘实际安装成功（InstallResult code=3，手环列表可见）
- 认证与推送协议为 astrobox 的 **WearPacket 协议**（非 Gadgetbridge 的 Command 协议）

## 架构

```
src-tauri/src/
├── main.rs / lib.rs       # Tauri 入口，注册 commands
├── commands.rs            # Tauri command 桥接（scan/connect/authenticate/install）
├── events.rs              # 前端事件名（install:progress）
├── ble/
│   ├── scanner.rs         # 设备扫描（bluer，过滤 mi/band/xiaomi）
│   ├── connection.rs      # SPP 连接管理（RFCOMM ch5）+ 认证会话保存
│   └── errors.rs          # BleError（thiserror）
└── protocol/
    ├── consts.rs          # 协议常量（唯一来源 docs/protocol-notes.md）
    ├── encoding.rs        # V2 帧 / CRC16 / protobuf / AES-CTR/CCM / HMAC / WearPacket
    ├── auth.rs            # authkey 认证握手（返回 Session）
    └── watchface.rs       # bin 解析 + WearPacket 安装 + MASS 分片推送
```

## 开发

```bash
npm install          # 前端依赖（NODE_ENV=production 时需 --include=dev --no-audit）
cd src-tauri
cargo test           # 27 个单测（协议 golden 向量 / 帧编解码 / 分块逻辑）
cd ..
npm run tauri dev    # 开发运行
```

## 使用说明

### 安装前置条件

1. **已绑定手环 + 有效 authkey**：用官方 App（小米运动健康）绑定手环，从手机日志提取 authkey（16 字节 hex，32 字符）；手环恢复出厂 / App 内解绑后 authkey 失效，需重新绑定提取。
2. **断开手机连接**：使用工具前关闭手机蓝牙或从手机断开手环（蓝牙独占）。
3. Linux 需 BlueZ（`bluetoothd` 运行中）与 dbus 权限。

### 操作步骤

1. **扫描**：点击「扫描设备」，选择列表中的 `Xiaomi Smart Band 10 Pro`。
2. **认证**：输入 authkey（32 位 hex，可带 `0x` 前缀），点击「连接并认证」。
   - 若手环此前未与电脑配对，需在手环/系统弹窗确认配对（DisplayYesNo）。
3. **安装**：选择 `.bin` / `.face` 表盘文件路径，点击「安装」，等待进度完成。
4. **完成**：手环表盘列表出现新表盘。

### 常见错误对照表

| 现象 | 原因 | 处理 |
|---|---|---|
| 扫描无设备 | 手环被手机占用 / 蓝牙关闭 | 关闭手机蓝牙，确认手环蓝牙开启 |
| 认证失败 "watch HMAC 验证失败" | authkey 错误 | 核对 authkey（32 hex 字符） |
| 认证失败（等待应答超时） | 手环未绑定 / 未配对 | 用官方 App 重新绑定并提取新 authkey；确认手环已配对 |
| 安装失败 "文件头部 magic 应为 5A A5" | 非表盘 bin | 使用合法 .bin/.face 表盘文件 |
| 安装失败 "InstallResult code=1" | 表盘与设备不兼容 / 已满 | 更换表盘；查看手环剩余空间 |
| 推送中断 | 蓝牙断开 | 重新连接后重试（进度事件已按分块上报） |

## 阶段 1 成果

- 确认 Band 10 Pro 的 V2 协议走**经典蓝牙 SPP 通道**（RFCOMM ch5），非 BLE GATT
- 认证 + 表盘安装完整跑通（authkey 认证 → WearPacket 协议 → MASS 分片上传）
- 表盘实际安装成功（InstallResult code=3，手环列表可见）
