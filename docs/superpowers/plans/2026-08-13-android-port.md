# 小米手环 10 Pro 表盘安装工具 —— Android 版（移动端）Implementation Plan

> **目标**：在 Android 手机端实现表盘直装工具（替代有问题的 AstroBox）。复用 Linux 版已真机验证的协议层（V1 Hello → V2 认证 → WearPacket 安装 → 列表确认/存储查询），重写蓝牙连接层（Linux bluer → Android 蓝牙 API）。

## 背景与约束

- 设备：Xiaomi Smart Band 10 Pro（M2551B1，固件 3.101.036），BLE 地址 2C:0D:CF:73:D9:95
- 协议：V2 协议走**经典蓝牙 SPP（RFCOMM ch5）**，认证/安装用 astrobox WearPacket 协议（见 docs/protocol-notes.md）
- 测试手机：**Android 16**（用户提供），手机通过官方 App 绑定手环（bonding 存在，手机可直接 RFCOMM 连接）
- authkey：用户自行提取（测试用 fdbde32f06e7f5cfe17d25c6b3d22b91）
- **Linux 版依赖 bluer（BlueZ D-Bus）无法在 Android 编译/运行** → 连接层必须重写
- 协议层（认证/上传/确认/存储）为纯 Rust，**可 100% 复用**（抽象出传输 trait）

## 技术架构

```
┌─ Kotlin（Android 蓝牙层，新写）──────────────┐
│  BluetoothAdapter → RFCOMM socket (ch5)     │
│  InputStream/OutputStream → JNI 暴露字节流   │
└──────────────┬──────────────────────────────┘
               │ JNI (Rust <-> Kotlin)
┌──────────────▼──────────────────────────────┐
│ Rust 协议层（复用 Linux 版）                │
│  SppTransport trait：read/write             │
│  auth::authenticate / watchface::push       │
└──────────────┬──────────────────────────────┘
               │ Tauri command
┌──────────────▼──────────────────────────────┐
│ React 前端（复用 Linux 版，UI 微调）        │
└─────────────────────────────────────────────┘
```

## 环境

- JDK 17+（用户级安装 ~/.local/jdk，无 sudo）
- Android SDK commandline-tools（~/Android/Sdk，sdkmanager 装 platform 36 + build-tools + NDK）
- Rust target：aarch64-linux-android（rustup）
- cargo-ndk（交叉编译 .so）
- Tauri 2 android（tauri android init/build）

---

## Task 1: 环境搭建（用户级，无 sudo）

- [ ] **Step 1**: 安装 JDK 17（Adoptium tarball → ~/.local/jdk，export JAVA_HOME）
- [ ] **Step 2**: 解压 commandline-tools → ~/Android/Sdk/cmdline-tools/latest
- [ ] **Step 3**: sdkmanager 安装 `platforms;android-36`、`build-tools;36.x`、`platform-tools`、`ndk;27.x`
- [ ] **Step 4**: rustup target add aarch64-linux-android；cargo install cargo-ndk
- [ ] **Step 5**: 验证：`cargo ndk -t arm64-v8a build` 能编译一个最小 Rust lib

## Task 2: Tauri Android 工程初始化

- [ ] **Step 1**: `npm run tauri android init`（生成 gen/android 工程）
- [ ] **Step 2**: 确认 tauri.conf.json bundle 含 android 目标
- [ ] **Step 3**: `npm run tauri android build -- --apk` 能产出 APK（先不管 bluer 依赖）

## Task 3: 传输抽象层（Rust trait）

- [ ] **Step 1**: 定义 `trait SppTransport { async fn read/write }`
- [ ] **Step 2**: Linux 实现（bluer Stream，现有代码包装）
- [ ] **Step 3**: Android 实现（JNI 桥到 Kotlin 字节流，通过 tauri plugin / jni crate）
- [ ] **Step 4**: 协议层（auth/watchface）改为泛型 over SppTransport，不直接依赖 bluer

## Task 4: Kotlin 蓝牙层

- [ ] **Step 1**: 权限（Android 12+ BLUETOOTH_CONNECT/SCAN + 位置；Manifest 声明）
- [ ] **Step 2**: RFCOMM 连接：`adapter.getRemoteDevice(addr).createRfcommSocketToServiceRecord(SPP_UUID)` → connect
- [ ] **Step 3**: 字节流：socket.inputStream/outputStream，JNI 暴露（或 ParcelFileDescriptor fd）
- [ ] **Step 4**: 断开/错误处理

## Task 5: POC 真机验证（Android 连手环）

- [ ] **Step 1**: 手机绑定手环（用户重绑），提取有效 authkey
- [ ] **Step 2**: Android 连接手环 → V1 Hello → 版本响应
- [ ] **Step 3**: V2 认证握手成功（PhoneNonce → WatchNonce → AuthStep3）
- [ ] **Step 4**: 记录 Android 侧连接/认证时序，与 Linux 对比

## Task 6: 协议集成（复用 Linux 版）

- [ ] **Step 1**: auth/watchface 通过 SppTransport 跑通（Android 后端）
- [ ] **Step 2**: 存储查询 GET_STORAGE_INFO / 列表确认 GET_INSTALLED_LIST
- [ ] **Step 3**: MASS 上传（BATCH 策略 Android 实测：手环 ACK 批级，用大 BATCH + 等 ACK）

## Task 7: 前端适配

- [ ] **Step 1**: 复用 Linux 版 React UI（连接/安装页）
- [ ] **Step 2**: 文件选择 → Android SAF（Storage Access Framework）或外部存储路径
- [ ] **Step 3**: 存储显示（复用 get_storage_info）

## Task 8: 端到端真机验证

- [ ] **Step 1**: 手机装 APK → 连接手环 → 认证 → 装表盘 → 手环显示新表盘
- [ ] **Step 2**: 错误路径（错误 authkey、损坏文件、中断）
- [ ] **Step 3**: 存储/列表确认
- [ ] **Step 4**: 提交 + 合并

---

## Self-Review

- **架构可行性**：协议层纯 Rust 可复用；连接层 Android API（RFCOMM）为平台标准做法；JNI 桥是 Rust↔Kotlin 的标准方案
- **风险**：
  - Android 经典蓝牙（BR/EDR）在部分设备上有连接限制（需真机验证）
  - JNI 桥的字节流性能（SPP 吞吐 ~18KB/s 手环限速，JNI 开销可忽略）
  - Android 16 权限模型（BLUETOOTH_CONNECT 等需运行时申请）
- **协议数据真实性**：全部复用 Linux 版已验证的 protocol-notes.md 值，无臆造
- **验收**：Task 8 真机装表盘成功（手环显示新表盘 + 存储查询正确）
