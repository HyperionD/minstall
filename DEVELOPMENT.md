# minstall 开发手册（AI 后续开发必读）

> 本文档整合了本项目全部研究、实现与踩坑经验，供后续开发者（含 AI）快速建立完整上下文。
> 涵盖：项目目标 → 协议逆向结论 → 架构设计 → 双平台实现 → 真机调试 → 已知问题与坑。
> 阅读顺序建议：§1（全貌）→ §2（协议）→ §3（架构）→ §4（平台实现）→ §5（调试）→ §6（坑速查）。

---

## 1. 项目全貌

**目标**：通过蓝牙直连小米手环 10 Pro（不经过官方 App），安装 `.bin`/`.face` 表盘文件。

**阶段**：
- 阶段 1（2026-08-12）：Python POC 协议验证 ✅
- 阶段 2（2026-08-13）：Tauri Linux GUI，真机验证（Task 16 全项）✅
- 阶段 3（2026-08-14）：Android 版，真机全链路（连接/认证/安装/断开）✅

**代码结构**：
```
src/                       # React 前端（Tauri 共享，平台微调）
src-tauri/src/
├── main.rs / lib.rs       # Tauri 入口，注册 commands
├── commands.rs            # command 桥接（scan/connect/authenticate/install/...）
├── events.rs              # 前端事件名
├── ble/                   # 蓝牙层（平台相关）
│   ├── connection.rs      #   Linux：bluer SPP
│   ├── connection_android.rs # Android：JNI + 阻塞读线程字节流
│   ├── scanner.rs / scanner_android.rs
│   ├── file_picker_android.rs  # Android SAF 文件选择
│   ├── authkey_android.rs      # Android authkey 自动读取
│   └── errors.rs
└── protocol/              # 协议层（纯 Rust，平台无关）
    ├── consts.rs          #   协议常量（唯一来源，勿臆造）
    ├── encoding.rs        #   V2 帧/CRC16/protobuf/AES/HMAC/WearPacket
    ├── auth.rs            #   authkey 认证握手
    └── watchface.rs       #   bin 解析 + 安装 + MASS 分片
pocs/                      # Python POC（scan/auth/spp_fast/install）
```

**核心设计原则**：**协议层纯 Rust 可 100% 复用，仅连接层按平台重写**。传输抽象为 `SppChannel<S: AsyncRead + AsyncWrite + Unpin>`，Linux 传 `bluer::rfcomm::Stream`，Android 传 `AndroidStream`。

---

## 2. 协议（真机验证结论，勿臆造）

### 2.1 通道：SPP（经典蓝牙），不是 BLE

- **V2 协议全部走经典蓝牙 SPP 通道（RFCOMM channel 5）**——这是最重要的发现
- BLE 5e/5f 特征写帧无响应（通道错误），BLE 仅用于配对身份
- SPP UUID：`00001101-0000-1000-8000-00805f9b34fb`
- SPP 通道可绕过 BLE bonding 限制（电脑端无需手环解绑即可连接认证）

### 2.2 协议族：astrobox 的 WearPacket（非 Gadgetbridge 的 Command）

- 认证帧与 Gadgetbridge Command 协议**字节兼容**（字段编号巧合重叠），但**消息语义不同**
- **表盘安装必须用 WearPacket**：`WearPacket{type=1, id=2, payload oneof}`，payload 字段编号：Account=3, System=4, WatchFace=6, **Mass=24（不是 7！）**
- WearPacket 认证：`type=ACCOUNT(1), id=AUTH_VERIFY(26)` → `AUTH_CONFIRM(27)`

### 2.3 V2 帧格式（小端）

```
[0..1] preamble 0xA5 0xA5
[2]    packet type 低 nibble：1=ACK, 2=SESSION_CONFIG, 3=DATA
[3]    sequence number (u8)
[4..5] payload length (u16 LE)
[6..7] CRC-16/ARC of payload (poly 0x8005, init 0, 无 xor, refin, refout)
[8..]  payload
```

DATA 包 payload：`[channel u8 低 nibble][opCode u8][body]`
- channel：1=PROTOBUF（加密），2=DATA（明文），5=ACTIVITY（加密）
- opCode：1=PLAINTEXT, 2=ENCRYPTED
- 加密：encryptV2 = AES-128-CTR（key 即 IV）；decryptV2 同

### 2.4 认证流程（authkey 绑定状态下）

```
1. V1 Hello（必需前置）：badcfe00c00300000100ef → 版本响应
2. START_SESSION_REQUEST（seq=0）→ START_SESSION_RESPONSE
3. PhoneNonce（明文，16B 随机）→ WatchNonce（61B: nonce16 + hmac32）
4. deriveSession(secret, phoneNonce, watchNonce) → 64B：
   (0-15)decKey (16-31)encKey (32-35)decNonce (36-39)encNonce
   = HMAC-SHA256(key=phoneNonce||watchNonce, msg=secret) → intermediate；
     再 HMAC-SHA256(key=intermediate, msg=tmp||"miwear-auth"||counter) 计数器扩展
5. verifyWatchHmac：HMAC-SHA256(key=decKey, msg=watchNonce||phoneNonce) == watchHmac
6. AuthStep3（明文，encryptV1=AES-128-CCM, nonce=(encNonce4,4×0x00,counter), macBits=32）
   → 手环回 type=1 subtype=27 → 认证完成
```

**认证前提：手环必须"已绑定"**——固件里有有效绑定记录（authkey 匹配）。恢复出厂/App 解绑 → 返回 `error_code=NO_BOUND(4)`。

### 2.5 表盘安装流程（WearPacket）

```
1. WatchFace PREPARE_INSTALL（type=4, id=4, prepare_info=6{id, size, version_code=65536}）
   → prepare_status=5（0=READY）
2. Mass PREPARE（type=22, id=0, prepare_request=1{data_type=16, data_id=md5, data_length}）
   → prepare_response=2{prepare_status=2, expected_slice_length=5}（真机 12288）
3. MASS 分片上传（DATA 通道明文）：L2[ch=2(Mass)][op=1(Write)][total u16][cur u16][fragment]
   fragment = expected_slice_length - 6（L2 头 2B + total/cur 4B）
   MassPacket：`[comp 0x00][type=16][md5 16B][size u32 LE][bytes] + crc32 u32 LE`
   **BATCH=2 稳定，>2 手环断连**（TX_WIN=3）
4. 等手环推送 InstallResult（type=4, id=5, install_result=7{id, code}）：
   code 2=SUCCESS, 3=INSTALL_USED（已安装）
5. GET_INSTALLED_LIST（type=4, id=0）查列表确认
```

**存储查询**：`WearPacket{type=SYSTEM(2), id=GET_STORAGE_INFO(62)}` → `storage_info=44{used,total}`（真机 used=12.62MB / total=259.38MB）。

### 2.6 Bin 表盘文件格式

- magic：fw[0]=0x5A, fw[1]=0xA5
- id：offset 0x28 起 null-terminated ASCII 数字串
- name：offset 0x68 起 null-terminated；若为 0xFFFFFFFF 则走 i18n 表

### 2.7 连接身份机制

手环 BLE 连接是**身份识别制**：绑定手机后拒绝陌生设备（静默，不进入配对）。"解绑"本质是清空手环侧配对身份。但 **SPP 通道可绕过该限制**（AstroBox 在 Linux 用 bluer 实现同样的 SPP Profile 连接）。

---

## 3. 架构设计

### 3.1 一套代码，双平台

```
前端 React（共用，平台微调）
        │ Tauri command
Rust 协议层（共用，平台无关）
        │ SppChannel<S: AsyncRead+AsyncWrite+Unpin>
┌───────┴───────┐
Linux bluer     Android JNI→Kotlin RFCOMM fd
cfg(linux)      cfg(android)
```

### 3.2 平台条件编译

- `Cargo.toml`：Linux only（bluer/futures/dialog），Android only（jni/libc）
- `ble/mod.rs`：`#[cfg(target_os)]` 声明 connection/scanner，Android `pub use ..._android as ...`
- `lib.rs`：dialog 插件仅 Linux；capabilities 的 `dialog:default` 限 desktop
- **capabilities 必须分平台**：`capabilities/default.json`（linux/macOS/windows）+ `capabilities/android.json`（android，含 core:default）
  - ⚠️ **Android 缺 core:default 会导致前端 listen 事件被 ACL 拒绝**（emit 返回 Ok 但前端收不到——静默失败，极难排查）

### 3.3 安装确认策略（体验优先）

手环推 InstallResult 不可靠（真机多次验证经常不推）。当前策略：
- MASS 上传完成后 **InstallResult 只等 10s**
- 收到 code=2/3 → `PushOutcome::Confirmed`
- 未收到 → **快速查一次列表**，确认到 → Confirmed；查不到 → `PushOutcome::Transferred`（前端提示"传输成功，请手环确认"）
- **绝不长时间等待**（300s 循环曾导致用户卡死）

---

## 4. 平台实现细节

### 4.1 Android 蓝牙层（Kotlin + JNI）

**数据流**：
```
Rust 协议层 ←→ AndroidStream (阻塞读线程 + channel) ←→ fd
                                               ↑
Kotlin BleRfcomm.connect(addr) → BluetoothSocket → 反射 mSocketFd
```

**Kotlin 类**（`gen/android/app/src/main/java/com/minstall/app/`）：
- `BleRfcomm.kt`：connect（RFCOMM socket→fd）、close（关闭 socket）、自动重配对（removeBond+createBond 顶掉官方 App 占用）
- `BleScan.kt`：startDiscovery + BroadcastReceiver（含已配对设备）
- `BleFilePicker.kt`：SAF 选文件，持久授权 URI，**不复制缓存，保留原始文件名**（映射存 SharedPreferences）
- `AuthkeyReader.kt`：剪贴板 + 扫描 Download/wearablelog 日志 zip，解析 `"encryptKey": "32hex"` 字段
- `AppContext.kt`：全局 context
- `MainActivity.kt`：init + 权限申请 + onActivityResult 转发

**Rust JNI 桥**：
- `Java_com_minstall_app_BleRfcomm_initJni`：保存 JavaVM
- `find_app_class`：**native 线程必须用缓存 ClassLoader.loadClass**（FindClass 在 native 线程找不到应用类，且失败会留 pending exception 导致闪退）

### 4.2 Android 传输层关键决策

**BluetoothSocket 的 fd 是阻塞模式**，不适合 AsyncFd（要求非阻塞）。当前方案：
- spawn 独立**阻塞读线程**（libc::read 循环）→ mpsc channel 送数据
- 写路径直接 libc::write（协议层逐批等 ACK，写窗口小）
- **fd 所有权归 Kotlin**：Rust 只借用（`i32`），断开由 Kotlin `BleRfcomm.close()` 关闭
  - ⚠️ 若 Rust `OwnedFd::from_raw_fd` 接管后 close → **fdsan abort**（`expected to be unowned, actually owned by ParcelFileDescriptor`）

### 4.3 Android 权限

- BLUETOOTH_CONNECT / BLUETOOTH_SCAN（Android 12+）
- ACCESS_FINE_LOCATION（<12）
- MANAGE_EXTERNAL_STORAGE（读导出日志，需设置页手动授权）
- **R8 keep 规则**：JNI 桥类必须 keep（`proguard-rules.pro`），否则 release 构建方法被裁 → NoSuchMethodError

### 4.4 Linux 蓝牙层（bluer）

- SPP Profile：`MessageBus(negotiate_unix_fd=True)`（**必须**，否则报 fd 传输错误）
- **用 dbus-fast**（dbus-next 的 Profile 方法签名无法正确暴露）
- DisplayYesNo agent（手环配对需要确认）
- ConnectProfile 返回错误不致命（br-connection-refused 后 ConnectRequest 仍到达）

### 4.5 前端（React + Tauri）

- 签名元素：圆形表盘 SVG（状态/进度可视化）
- 深色 + 浅色双主题（CSS 变量 + `prefers-color-scheme`）
- 记住上次连接（localStorage：MAC + authkey）
- authkey 自动检测（剪贴板/日志）+ 手动输入

---

## 5. 真机调试方法

### 5.1 构建 APK + 签名

```bash
export JAVA_HOME=~/.local/jdk/jdk-17.0.20+8
export ANDROID_HOME=~/Android/Sdk
npm run tauri android build -- --apk      # 产物 app-universal-release.apk
# release 签名由 gradle 自动读取 keystore.properties（不含密钥，已 gitignore）
```

⚠️ **前端资源嵌入缓存**：改前端后需 `rm -rf src-tauri/target/aarch64-linux-android/release/build/minstall-*/`，否则 APK 里是旧前端（tauri-codegen 缓存未失效）。

### 5.2 查看日志

```bash
adb logcat -d | grep -E "RustStdoutStderr|BleScan|BleRfcomm|AuthkeyReader|BleFilePicker"
adb logcat -d -b crash   # 崩溃
# 协议层诊断日志带 [minstall] 前缀（eprintln!）
```

### 5.3 系统蓝牙诊断

```bash
adb shell "dumpsys bluetooth_manager | grep -iE 'RFCOMM Connection|MOST_FEQ_ADDR|bond'"
# MOST_FEQ_ADDR 显示哪个 app 在占用手环（com.mi.health = 官方 App 抢占）
```

### 5.4 常见验证路径

- 事件系统：`app.emit` 返回 Ok 但前端收不到 → 检查 capabilities 是否含 core:default（Android）
- 协议卡住：wait_wp 用 200ms timeout 包裹 read_more（SPP 读无数据时永久阻塞）

---

## 6. 坑速查表（按症状 → 根因 → 解决）

| 症状 | 根因 | 解决 |
|---|---|---|
| BLE 写帧无响应 | 通道错误（V2 走 SPP 非 BLE） | 改走 SPP（RFCOMM ch5） |
| 认证 NO_BOUND | 手环未绑定 | 官方 App 重新绑定 + 提取新 authkey |
| 手环"配对失败" | NoInputNoOutput agent | 用 DisplayYesNo agent 自动确认 |
| 上传中途断连 | BATCH>2 | BATCH=2 |
| 数据传完表盘不出现 | 只看发送成功，没等 InstallResult | 等手环推送 InstallResult + 列表兜底 |
| Mass PREPARE 无响应 | 用 7 当 Mass 字段号 | Mass=24 |
| 认证收不到 START_SESSION_RESPONSE | V2Accumulator 双重消费 | feed 只累积不解析，drain 单独取 |
| Linux 白屏 | Node 17+ localhost→IPv6 ::1 | vite.config.ts 强制 host=127.0.0.1 |
| Android 闪退（扫描/连接 ClassNotFound） | native 线程 FindClass 失败 | find_app_class 用缓存 ClassLoader |
| 进度事件 emit ok 但前端收不到 | Android 无 core:default capability | 新增 capabilities/android.json |
| release 构建 NoSuchMethodError | R8 裁剪 JNI 桥类 | proguard keep BleScan/BleRfcomm/BleFilePicker/AuthkeyReader |
| 安装确认阶段卡死（timeout 不触发） | 阻塞 fd + AsyncFd | 独立阻塞读线程 + channel |
| 断开即崩溃（fdsan abort） | Rust close 了归 Kotlin 的 fd | Rust 只借用 fd，断开由 Kotlin close() |
| 手环已配对扫描不到 | startDiscovery 不返回 bonded | 扫描并入 adapter.bondedDevices |
| 连接失败 IOException read -1 | 官方 App 占用 RFCOMM | 自动 removeBond + createBond 顶掉 |
| 安装无完成反馈（卡 300s） | InstallResult 不推 + 列表兜底太长 | 10s 短等 + 返回「已传输」 |
| 无法连续安装第二个表盘 | CountDownLatch 单例耗尽 | 每次 pick() 新建 latch |
| 前端资源没更新 | tauri-codegen 缓存 | 清理 build/minstall-* 目录 |

---

## 7. 待办 / 已知限制

- 安装「已确认」提示：当前手环不推 InstallResult 时返回「已传输」，可考虑让用户手动确认收尾
- 手环反复安装会累积存储（覆盖不彻底），建议定期用官方 App 清理
- authkey 与手环绑定状态关联；重新绑定后需重新提取
- 发布需注意：工具绕过官方 App（非官方协议逆向），仅供个人研究/自用；建议 GitHub 自托管分发，不上应用商店
