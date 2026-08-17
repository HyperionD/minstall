# Contributing

感谢参与 minstall。项目当前是面向小米手环 10 Pro 的 experimental research tool。

## 开始开发

请先阅读：

1. [README](README.md)
2. [DEVELOPMENT.md](DEVELOPMENT.md)
3. [安全报告说明](SECURITY.md)

基本检查：

```bash
npm ci
npm run build
cd src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## 提交变更

- 一个 PR 只解决一个主题。
- 不要提交 authkey、设备日志、私有表盘文件、签名密钥或构建产物。
- 新增行为应同时添加测试或说明无法自动化测试的硬件条件。
- 提交信息建议使用 Conventional Commits，例如 `fix(android): ...`。
- 涉及协议或安全行为的改动请在 PR 描述中说明验证设备、固件和测试结果。

## Pull Request

请说明变更内容、验证命令、已知限制，以及是否涉及 Android 真机验证。不要在 Issue 或 PR 中粘贴 authkey 或完整导出日志。

## 发布 Android 版本

推送符合 `v*.*.*` 格式的 tag 后，GitHub Actions 会自动构建并发布 Android ARM64 pre-release。也可以在 Actions 页面手动运行 `Release Android APK`，输入已有版本 tag。

仓库需要配置以下 GitHub Actions Secrets；签名文件只通过 `ANDROID_KEYSTORE_BASE64` 注入，不要提交到仓库：

- `ANDROID_KEYSTORE_BASE64`
- `MINSTALL_STORE_PASSWORD`
- `MINSTALL_KEY_ALIAS`
- `MINSTALL_KEY_PASSWORD`
