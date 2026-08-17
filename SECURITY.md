# Security Policy

## 报告安全问题

请不要在公开 Issue 中发布 authkey、设备 MAC、导出日志或可复现绑定凭据。

优先通过 GitHub Security Advisories 私下报告；如果仓库尚未启用该功能，请联系仓库维护者 `HyperionD` 后再提供最小化复现信息。

## 敏感信息处理

- authkey 是与手环绑定相关的敏感凭据。
- 应用不会上传 authkey 或设备数据。
- “记住 authkey”使用 Linux Secret Service 或 Android Keystore；安全存储不可用时不会退回明文文件。
- 反馈问题时请先删除 authkey、MAC 和官方 App 导出日志。

## 支持范围

本项目目前只承诺 Linux 桌面和 Android 的已验证路径。蓝牙协议行为和厂商固件可能发生变化。
