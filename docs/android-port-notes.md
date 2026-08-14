# 手环 10 Pro 表盘直装 —— Android 版移植笔记（2026-08-13）

> 背景：用户在手机端需要替代有问题的 AstroBox 的工具。Linux 版（bluer + SPP）已真机验证完成（Task 16），本笔记记录 Android 版移植的架构、实现与踩坑。

## 1. 为什么不能直接编译到 Android

- 核心依赖 `bluer` **完全依赖 Linux BlueZ 的 D-Bus 接口**（`org.bluez`、dbus-crossroads、dbus-tokio）
- Android 没有 BlueZ/D-Bus，用的是自己的蓝牙 HAL（BluetoothSocket API）
- `bluer` 的依赖 `libdbus-sys` 在 Android target 编译失败（无 libdbus C 库）
- 手环协议走 **SPP（RFCOMM ch5）**，Android 上是完全不同的 API

**结论：连接层必须重写，协议层可 100% 复用**（协议是纯 Rust：V1 Hello → V2 认证 → WearPacket 安装 → 列表确认 → 存储查询）。

## 2. 架构：一套代码，双平台

```
┌─ 前端 React（共用，平台微调）─────────────┐
│  扫描/MAC 输入 / 文件路径 / 存储显示      │
└──────────────┬────────────────────────────┘
               │ Tauri command
┌──────────────▼────────────────────────────┐
│ Rust 协议层（共用，平台无关）             │
│  auth / watchface::push / query_storage   │
└──────────────┬────────────────────────────┘
               │ SppChannel<S: AsyncRead+AsyncWrite+Unpin>
┌──────────────┼────────────────────────────┐
│ Linux        │  Android                   │
│ bluer Stream │  JNI → Kotlin RFCOMM fd    │
│ cfg(linux)   │  cfg(android)              │
└──────────────┴────────────────────────────┘
```

### 关键重构：传输抽象层（tokio trait）

- `SppChannel<'a, S>` 泛型化 over `S: tokio::io::AsyncRead + AsyncWrite + Unpin`
- `auth::authenticate(stream, ...)` / `watchface::push(stream, session, seq_ref, ...)` / `query_storage(stream, session, seq_ref, ...)`
  **不再依赖 Manager/bluer**，只接收字节流 + seq 引用
- Linux 传 `bluer::rfcomm::Stream`（已实现 tokio traits）；Android 传 `AndroidStream`（fd 包装）
- seq 由调用方（commands.rs）从 Manager 取、操作后写回

### 平台条件编译

- `Cargo.toml`：
  - Linux only：`bluer`、`futures`、`tauri-plugin-dialog`
  - Android only：`jni`、`libc`
- `ble/mod.rs`：`#[cfg(target_os)]` 声明 `connection`/`scanner`，Android 用 `pub use ..._android as ...` 统一命名
- `lib.rs`：dialog 插件仅 Linux 注册；capabilities 的 `dialog:default` 限 `platforms: [linux, macOS, windows]`
  （Tauri 移动端插件需手动配 gradle，且 Android 文件选择应走 SAF，故 Android 不用 dialog 插件）

## 3. Android 蓝牙层（Kotlin + JNI）

### 数据流

```
Rust 协议层 ←→ AndroidStream (tokio AsyncFd<OwnedFd>) ←→ fd
                                                          ↑
Kotlin BleRfcomm.connect(addr) → BluetoothSocket → 反射拿 mSocketFd
```

### Kotlin（gen/android/app/src/main/java/com/minstall/app/）

- **BleRfcomm.kt**：`connect(addr): Int` —— `createRfcommSocketToServiceRecord(SPP_UUID)` → `connect()` → 反射 `mSocketFd` 返回 fd；失败返回 -1
- **BleScan.kt**：`scan(timeoutMs): Array<String>` —— `startDiscovery` + BroadcastReceiver 收集设备（`"name|address|rssi"`），CountDownLatch 等待
- **AppContext.kt**：全局 applicationContext（MainActivity 设置，供 registerReceiver 用）
- **MainActivity.kt**：启动时 `BleRfcomm.init()`（加载 native + 保存 JavaVM）+ 蓝牙运行时权限申请

### Rust JNI 桥（ble/connection_android.rs / scanner_android.rs）

- `Java_com_minstall_app_BleRfcomm_initJni`：JNI 入口保存 JavaVM 到全局 `OnceLock`
- `jni_connect`：attach 当前线程 → 调 `BleRfcomm.connect(addr)` → 拿 fd
- `AndroidStream`：`OwnedFd` → `tokio::io::unix::AsyncFd`，用 libc read/write 实现 AsyncRead/AsyncWrite
- `scanner_android::scan`：`spawn_blocking` 内 JNI 调 `BleScan.scan`，解析设备数组 + `filter_relevant`

## 4. 环境搭建（用户级，无 sudo）

| 组件 | 方式 |
|---|---|
| JDK 17 | Adoptium tarball → `~/.local/jdk`（清华镜像，1.8MB/s） |
| Android SDK | commandline-tools → `~/Android/Sdk`（腾讯镜像下载 zip） |
| platform-tools / platform-36 / build-tools-36 | `sdkmanager`（Google 直连，网络抖动时重试） |
| NDK 27.1 | sdkmanager 太慢 → 手动 `curl` Google 直连（~775KB/s）解压到 `ndk/27.1.12297006` + 写 source.properties |
| Rust targets | `rustup target add aarch64-linux-android`（tauri 自动补 i686/x86_64） |
| cargo-ndk | `cargo install cargo-ndk` |

**注意**：
- sdkmanager 下载慢/失败多为网络抖动，可手动 curl 替代
- NDK zip 解压后有 `android-ndk-r27b` 嵌套目录，需上移（否则 tauri 找不到 toolchains）

## 5. 构建 APK

```bash
export JAVA_HOME=~/.local/jdk/jdk-17.0.20+8
export ANDROID_HOME=~/Android/Sdk
npm run tauri android build -- --apk     # 产出 unsigned APK
# 签名（测试用 debug keystore）
~/Android/Sdk/build-tools/36.0.0/apksigner sign \
  --ks ~/.android/debug.keystore --ks-pass pass:android --key-pass pass:android \
  --out .../minstall-debug.apk .../app-universal-release-unsigned.apk
```

APK 产物：`src-tauri/gen/android/app/build/outputs/apk/universal/release/minstall-debug.apk`

## 6. 踩坑记录（真机/编译）

| 坑 | 现象 | 解决 |
|---|---|---|
| libdbus-sys Android 编译失败 | bluer 依赖无 libdbus | bluer/futures/dialog 移入 `cfg(linux)` target 依赖 |
| dialog 权限 Android ACL 找不到 | `Permission dialog:default not found` | capabilities `dialog:default` 限 desktop 平台；Android 文件选择走 SAF |
| NDK 嵌套目录 | tauri 找不到 toolchains/llvm | 解压后把 `android-ndk-r27b/*` 上移到 NDK 版本目录 |
| 扫描提示 JavaVM 未初始化 | BleRfcomm.init 未触发（扫描先于对象初始化） | MainActivity.onCreate 显式 `BleRfcomm.init()` |
| 点扫描闪退 | Android 13+ registerReceiver 无 flags 抛异常 | registerReceiver 带 `RECEIVER_NOT_EXPORTED`；scan 整体 try-catch |
| Kotlin lateinit 跨文件访问 | backing field 不可访问 | 改 `@Volatile var ctx: Context?` |
| jni crate 数组 API | JObject 不支持 get_array_length | 用 `JObjectArray` 类型 |
| native 线程 FindClass 失败 | 扫描/连接 ClassNotFoundException 闪退 | 缓存应用 ClassLoader，native 线程用 loadClass 兜底（find_app_class） |
| 事件监听被 ACL 拒绝 | 进度事件 emit ok 但前端收不到 | capabilities 新增 android.json（含 core:default，桌面限 linux/macOS/windows） |
| R8 裁剪 JNI 桥类 | 类存在但方法被裁，NoSuchMethodError | proguard keep BleScan/BleRfcomm/BleFilePicker |
| BluetoothSocket fd 阻塞模式 | AsyncFd 要求非阻塞，阻塞 read 卡死 timeout | 独立阻塞读线程 + mpsc channel |
| fd 所有权冲突 | fdsan abort: close unowned fd（断开即崩） | Rust 只借用 fd，断开由 Kotlin `BleRfcomm.close()` 关闭 |
| startDiscovery 不含已配对设备 | 手环已配对扫描不到 | 扫描结果并入 `adapter.bondedDevices` |
| RFCOMM 被官方 App 占用 | 连接失败 IOException read -1 | 失败自动 removeBond + createBond 重新配对顶掉占用 |
| 安装确认等太久 | 手环不推 InstallResult，卡 300s | 10s 短等 + 快速列表确认，未确认返回「已传输」由用户手环确认 |

## 7. 状态与下一步

**已完成**：
- 传输抽象层重构（Linux/Android 通用协议层）
- 平台条件编译，双平台编译通过（Linux 27 测试、Android 0 错误）
- Kotlin RFCOMM 连接 + JNI fd 桥 + 阻塞读线程字节流
- Kotlin 蓝牙扫描（startDiscovery + JNI，含已配对设备）
- 蓝牙权限（Manifest + 运行时申请）
- 自动解除配对顶掉 RFCOMM 占用方
- SAF 文件选择（ACTION_OPEN_DOCUMENT，持久授权 URI，不复制缓存，保留原始文件名）
- authkey 自动读取（剪贴板 + 自动扫描 Download/wearablelog 日志 zip，解析 encryptKey 字段）
- **真机全链路验证（2026-08-14）：连接 → 认证 → 安装 → 断开**
- 产品级 UI（表盘签名元素 + 深色/浅色双主题）+ 自定义图标
- 连续安装多个表盘（文件选择 latch 每次重建）

**待办**：
- [x] 签名正式化（release keystore，替代 debug）
- [ ] 安装成功后的「已确认」提示可考虑让用户手动确认收尾

## 8. 补充：8-14 Android 版真机联调修复（同日）

- **事件监听被 ACL 拒绝**：capabilities 缺 android.json（无 core:default）→ 进度事件 emit ok 但前端收不到；新增 `src-tauri/capabilities/android.json`（含 core:default，桌面限 linux/macOS/windows）
- **native 线程 FindClass 失败**：扫描/连接 ClassNotFoundException → 缓存应用 ClassLoader，`find_app_class` 用 loadClass 兜底
- **R8 裁剪 JNI 桥类**：BleScan/BleRfcomm/BleFilePicker/AuthkeyReader 方法被裁 → proguard keep
- **BluetoothSocket fd 阻塞模式**：AsyncFd 需非阻塞 fd，阻塞 read 卡死 timeout → 独立阻塞读线程 + mpsc channel
- **fd 所有权**：Rust 只借用 fd（OwnedFd close 触发 fdsan abort），断开由 Kotlin `BleRfcomm.close()` 关闭
- **startDiscovery 不含已配对设备**：扫描并入 `adapter.bondedDevices`
- **RFCOMM 被官方 App 占用**：自动 removeBond + createBond 重新配对顶掉
- **安装确认等太久**：InstallResult 只等 10s，未收到返回「已传输」由用户手环确认
- **无法连续安装**：BleFilePicker CountDownLatch 单例耗尽，第二次 await 立即返回 → 每次新建 latch

## 8. 补充：8-13 Linux 版修复（同日）

- **Tauri 白屏根因**：Node 17+ `localhost` 默认解析为 IPv6 `::1`，Vite 只监听 `::1`；WebKitGTK webview 用 IPv4 `127.0.0.1` 连接失败 → 白屏。修复：`vite.config.ts` 强制 `server.host: "127.0.0.1"`
- **启动脚本** `start.sh`：清理残留进程（避免 Vite 端口 1420 冲突）+ 启动
- **UI 改进**：安装成功后留在安装页（保持认证状态，可连续安装/查存储）+ 绿色成功横幅；文件选择对话框（dialog 插件，仅桌面）；存储显示（get_storage_info）
- **协议侧**：安装确认双通道（InstallResult 快速 + GET_INSTALLED_LIST 兜底）已在 protocol-notes.md 5.1 记录
