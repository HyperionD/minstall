# minstall

小米手环 10 Pro 表盘直装工具（蓝牙直连安装 .bin/.face 表盘，不经过官方 App）。

- 阶段 1：POC 协议验证（`pocs/`，Python + dbus-fast）—— **已完成（2026-08-12）**
- 阶段 2：Tauri 跨平台 GUI（进行中）

## 文档

- [`docs/research-summary.md`](docs/research-summary.md) —— 研究历程与结论总结（人读）
- [`docs/protocol-notes.md`](docs/protocol-notes.md) —— 协议技术细节（真机验证数据）

## 阶段 1 成果

- 确认 Band 10 Pro 的 V2 协议走**经典蓝牙 SPP 通道**（RFCOMM ch5），非 BLE GATT
- 认证 + 表盘安装完整跑通（authkey 认证 → WearPacket 协议 → MASS 分片上传）
- 表盘实际安装成功（InstallResult code=3，手环列表可见）

## 使用前提

- 已通过第三方工具/官方 App 获取手环 authkey（16 字节 hex）
- 手环处于"已绑定"状态（恢复出厂/解绑后 authkey 失效，需重新绑定提取）
- 使用前断开手机与手环的连接
