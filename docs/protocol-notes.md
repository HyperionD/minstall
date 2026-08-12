# 手环 10 Pro BLE 协议笔记

> 本文件是 POC 阶段的核心产出。所有协议值必须来自真机验证或明确标注来源的参考实现，禁止臆造。
>
> **标注约定**：
> - `待真机确认`：来自参考实现（第 1 节的 Gadgetbridge / Kodo，基于 Band 9 / Band 9 Active），尚未经 Band 10 Pro 真机验证。
> - `真机确认`：已在 Band 10 Pro 真机（2C:0D:CF:73:D9:95）验证。当前验证范围：GATT 服务/特征枚举（2026-08-11，第 3 节）；协议语义（认证握手、表盘推送）待 Task 7/9 验证。

## 1. 参考实现来源

获取日期：2026-08-11。参考设备型号：Xiaomi Smart Band 9 / Band 9 Active（与 Band 10 Pro 同属小米手环系，协议版本 V2）。2026-08-11 真机 GATT 枚举已确认 Band 10 Pro 的 fe95 服务仅含 V2 特征（5e/5f），与参考实现一致；V1 特征（51/52/53/55）未出现（详见第 3 节）。

| 项目 | 仓库 | 关键文件 | 许可 | 说明 |
|---|---|---|---|---|
| Gadgetbridge | https://github.com/Freeyourgadget/Gadgetbridge | `app/src/main/java/nodomain/freeyourgadget/gadgetbridge/service/devices/xiaomi/services/XiaomiWatchfaceService.java`（表盘安装命令）、`.../xiaomi/services/XiaomiDataUploadService.java`（数据上传）、`.../xiaomi/devices/xiaomi/XiaomiFWHelper.java`（bin 解析）、`app/src/main/proto/xiaomi.proto`（protobuf 定义） | AGPL-3.0 | 支持 miband8 / miband8active / miband8pro / miband9 / miband9pro / redmiwatch3active 等 |
| Kodo | https://github.com/kidneyweakx/Kodo | `android-port/src/main/java/com/kidneyweakx/miband9active/xiaomi/protocol/XiaomiUuids.kt`、`.../protocol/XiaomiSppPacketV2.kt`、`.../protocol/MiBand9BleDriver.kt`、`.../auth/XiaomiAuthSession.kt` + `.../auth/XiaomiCrypto.kt`、`.../services/MiBand9DataUploader.kt`、`android-port/src/main/proto/xiaomi.proto` | AGPL-3.0 | Kotlin port，聚焦 Band 9 Active |

> 注意：AGPL-3.0 许可的参考实现代码不可直接复制进本项目；仅作协议参考。

## 2. 设备信息（真机）

| 项目 | 值 |
|---|---|
| 型号 | Xiaomi Smart Band 10 Pro |
| 广播名 | Xiaomi Smart Band 10 Pro D995（D995 为广播名内型号标识） |
| 蓝牙地址 | 2C:0D:CF:73:D9:95 |
| 固件版本 | 3.101.036（2026-08-12 加密通道读取 DeviceInfo 真机确认） |
| 设备类型 | M2551B1（DeviceInfo.deviceType 真机确认） |
| 表盘分辨率 | 待补充（以真机规格为准） |

## 3. GATT 服务枚举结果（真机，scan.py 产出）

下表为 2026-08-11 真机（Band 10 Pro，2C:0D:CF:73:D9:95）GATT 枚举结果（scan.py 产出，原始数据见 `.superpowers/sdd/2026-08-11-watchface-installer/task5-gatt-enum.json`）。“疑似用途”列对照参考实现（Kodo `XiaomiUuids.kt` / Gadgetbridge）标注。

| Service UUID | Characteristic UUID | Properties | 疑似用途（对照参考实现） |
|---|---|---|---|
| `00001800-0000-1000-8000-00805f9b34fb`（GAP，标准） | `00002a00-0000-1000-8000-00805f9b34fb` | read | 标准 GAP：Device Name |
| 同上 | `00002a01-0000-1000-8000-00805f9b34fb` | read | 标准 GAP：Appearance |
| 同上 | `00002a04-0000-1000-8000-00805f9b34fb` | read | 标准 GAP：Peripheral Preferred Connection Parameters |
| 同上 | `00002aa6-0000-1000-8000-00805f9b34fb` | read | 标准 GAP：Central Address Resolution |
| `00001801-0000-1000-8000-00805f9b34fb`（GATT，标准） | `00002a05-0000-1000-8000-00805f9b34fb` | indicate | 标准 GATT：Service Changed |
| 同上 | `00002b29-0000-1000-8000-00805f9b34fb` | read, write | 标准 GATT：Client Supported Features |
| 同上 | `00002b3a-0000-1000-8000-00805f9b34fb` | read | 标准 GATT：Database Hash |
| `0000fe95-0000-1000-8000-00805f9b34fb`（Xiaomi 私有） | `00000050-0000-1000-8000-00805f9b34fb` | read | 读取特征；参考实现未涉及，用途待确认 |
| 同上 | `0000005e-0000-1000-8000-00805f9b34fb`（V2 RX） | write-without-response, notify | V2 通道接收（读方向）——**真机确认**，与参考实现 V2 RX 一致 |
| 同上 | `0000005f-0000-1000-8000-00805f9b34fb`（V2 TX） | write-without-response, notify | V2 通道发送（写方向）——**真机确认**，与参考实现 V2 TX 一致 |
| `0000fdab-0000-1000-8000-00805f9b34fb`（自定义） | `00000001-0000-1000-8000-00805f9b34fb` | read | 参考实现未涉及，用途未知 |
| 同上 | `00000002-0000-1000-8000-00805f9b34fb` | write-without-response, notify | 参考实现未涉及，用途未知 |
| 同上 | `00000003-0000-1000-8000-00805f9b34fb` | write-without-response, notify | 参考实现未涉及，用途未知 |
| 同上 | `00000004-0000-1000-8000-00805f9b34fb` | read, notify | 参考实现未涉及，用途未知 |
| `0000180f-0000-1000-8000-00805f9b34fb`（Battery，标准） | `00002a19-0000-1000-8000-00805f9b34fb` | read, notify | 标准：Battery Level |
| `00001812-0000-1000-8000-00805f9b34fb`（HID，标准） | `00002a4a-0000-1000-8000-00805f9b34fb` | read | 标准 HID：HID Information |
| 同上 | `00002a4c-0000-1000-8000-00805f9b34fb` | read | 标准 HID：HID Control Point |
| `0000180a-0000-1000-8000-00805f9b34fb`（Device Information，标准） | `00002a50-0000-1000-8000-00805f9b34fb` | read | 标准：PnP ID（固件版本/设备信息读取入口） |
| `00003802-0000-1000-8000-00805f9b34fb`（自定义） | `00004a02-0000-1000-8000-00805f9b34fb` | read, write, notify | 参考实现未涉及，用途未知 |
| `cc353442-be58-4ea2-876e-11d8d6976366`（自定义） | `c551c36a-0377-4a29-9657-74ffb655a188` | read, write, notify | 参考实现未涉及，用途未知 |
| `0000180d-0000-1000-8000-00805f9b34fb`（Heart Rate，标准） | `00002a37-0000-1000-8000-00805f9b34fb` | notify | 标准：Heart Rate Measurement |
| `0000fd2d-0000-1000-8000-00805f9b34fb`（自定义） | `0000cf07-0000-0000-0000-000000000000` | write-without-response, notify | 参考实现未涉及，用途未知 |
| 同上 | `0000cf08-0000-0000-0000-000000000000` | write-without-response, notify | 参考实现未涉及，用途未知 |
| `1b7e8251-2877-41c3-b46e-cf057c562023`（自定义） | `8ac32d3f-5cb9-4d44-bec2-ee689169f626` | write-without-response, notify | 参考实现未涉及，用途未知 |

**与参考实现的对照结论**：

- **协议匹配**：fe95 服务含 `0000005e`（V2 RX）与 `0000005f`（V2 TX），属性均为 write-without-response + notify——与参考实现 V2 RX/TX 完全一致，**真机确认**。
- **差异**：V1 特征 `51/52/53/55`（COMMAND_READ / COMMAND_WRITE / ACTIVITY_DATA / DATA_UPLOAD）在真机上**未出现**——fe95 下仅 `50/5e/5f`。Band 10 Pro 走 V2 协议，无 V1 遗留特征。
- `00002902`（CCC）是特征配置描述符而非特征：scan.py 不枚举描述符，故未列入表；5e/5f 带 notify 属性意味着对应 CCC 描述符存在（enable notification 时写入）。
- 其余自定义服务（fdab / 3802 / cc353442 / fd2d / 1b7e8251）参考实现未涉及、用途未知，如需分析需逆向（超出当前 POC 范围）。

**操作经验（2026-08-11 真机）**：服务发现（GATT 枚举）前**必须先完成 BLE 配对**——NoInputNoOutput 配对代理配对后需在手环屏幕确认；未配对时 GATT 枚举结果为空（无任何服务）。此外设备需先从手机 App 解绑/断开（或关闭手机蓝牙），否则手环可能不与 POC 建立连接。

## 4. authkey 认证

- authkey 长度（hex 字符数）：32（16 字节）；"0x" 前缀可接受，待真机确认
- 认证 service UUID：`0000fe95-0000-1000-8000-00805f9b34fb`，**真机确认**（服务存在）
- 认证 characteristic UUID：V2 TX `...5f`（write-without-response）/ V2 RX `...5e`（notify），**真机确认**（两特征均带 write-without-response + notify 属性）
- 握手流程（步骤序列 + 每步帧格式）：

**V2 帧格式**（`XiaomiSppPacketV2.kt`，小端，待真机确认）：

```
[0..1] preamble 0xA5 0xA5
[2]    packet type（低 nibble）：1=ACK, 2=SESSION_CONFIG, 3=DATA
[3]    sequence number (u8)
[4..5] payload length (u16 LE)
[6..7] CRC-16/ARC of payload (u16 LE)（poly 0x8005, init 0, 无 xor, refin, refout）
[8..]  payload
```

**DATA 包 payload**：`[channel u8 低 nibble][opCode u8][body]`
- channel: 1=PROTOBUF（加密）, 2=DATA（明文）, 5=ACTIVITY（加密）
- opCode: 1=PLAINTEXT, 2=ENCRYPTED
- 加密方式：encryptV2 = AES-128-CTR，key 即 IV（legacy quirk）；decryptV2 同

**SessionConfig payload**：`[opcode u8][TLV...]`
- opcode: 1=START_SESSION_REQUEST（seq 固定 0）, 2=START_SESSION_RESPONSE, 3/4=STOP

**认证序列**（PROTOBUF 通道，加密前的明文阶段，待真机确认）：

1. 发 SessionConfig START_SESSION_REQUEST（seq=0, opcode=1）
2. 发 `Command{type=1, subtype=26(CMD_NONCE), Auth.PhoneNonce{nonce=16B 随机}}`（明文）
3. 收 `Command{type=1, subtype=26, Auth.WatchNonce{nonce=16B, hmac}}` → 验证
   - `deriveSession(secret, phoneNonce, watchNonce)` → 64B：(0-15)decKey (16-31)encKey (32-35)decNonce (36-39)encNonce
   - deriveSession：HMAC-SHA256(key=phoneNonce||watchNonce, msg=secret) → intermediate；再 HMAC-SHA256(key=intermediate, msg=tmp||"miwear-auth"||counter) 计数器扩展（counter 从 1 起）
   - verifyWatchHmac：HMAC-SHA256(key=decKey, msg=watchNonce||phoneNonce) == watchHmac
4. 发 `Command{type=1, subtype=27(CMD_AUTH), Auth.AuthStep3{encryptedNonces=HMAC-SHA256(key=encKey, msg=phoneNonce||watchNonce)（明文域，不加密）, encryptedDeviceInfo=encryptV1(AuthDeviceInfo{unknown1=0, phoneApiLevel=float, phoneName=str, unknown3=224, region=2字母大写})}}`（明文）
   - encryptV1 = AES-128-CCM，nonce = (encNonce4, 4×0x00, counter u32 LE)，macBits=32
5. 收 `type=1, subtype=27 或 5` → Connected（此后可发加密命令）

之后 PROTOBUF 通道命令 encrypt=true（opCode=ENCRYPTED），V2 加密。

### 4.1 真机验证中间状态（2026-08-11，Task 7 进行中）

**已完成**：
- 配对流程验证通过：注册 NoInputNoOutput agent（dbus，`_register_noinput_agent`）→ `connect` 后 BlueZ **自动完成配对**（Paired=True, ServicesResolved=True）→ 服务发现正常。多次复现。
- 配对后服务发现正常（fe95/5e/5f 可见，一次枚举到完整 12 服务/24 特征）。
- dbus 会话内 ATT 通道可用：read 2a00 成功（验证 GattCharacteristic1.ReadValue 可用）。
- `auth.py` 已实现 dbus 配对路径：`_register_noinput_agent`（注册+设为默认 agent）+ `_dbus_pair`（connect→pair→断开，bleak 复用配对状态）。

**卡点（未解决）**：
- `Bonded=False`：配对不持久化（无完整 bonding）。connect 自动配对为临时配对；`pair()` 报 "Already Paired" 无法触发完整 bonding；手环端曾显示"配对失败"。
- **手环不响应认证帧**：写入 START_SESSION_REQUEST（27B 与 16B 两种帧）后 RX 无任何通知；写操作本身成功。疑似：① 未加密连接（Bonded=False）下手环忽略命令；② ATT MTU 仍为 23，27B 帧可能未完整送达；③ 帧 TLV 参数需调整。
- **bleak 连接后 ATT 读写报 "Not connected"**（BlueZ 后端连接状态与 bleak 报告不一致）；dbus 会话内读写正常 → **认证握手应改用纯 dbus 读写**（当前 auth.py 仍走 bleak，需改）。
- BlueZ 5.82 + Intel 9560 环境连接不稳定：设备/服务对象时有时无（`br-connection-create-socket` 错误偶发）。

**下一步（明天继续）优先级**：
1. auth.py 认证传输改为纯 dbus（枚举特征→StartNotify(5e)→WriteValue(5f)），在**同一 dbus 会话**完成握手
2. 若仍无响应：抓包（btmon，需 sudo）确认帧是否送达；检查 MTU 协商（读 5e/5f 属性或 exchange）；排查 bonding（尝试 unpair 后重新 pair，或 bluetoothctl/桌面工具触发完整 bonding）
3. 考虑换 Android 手机验证（Android 系统配对/加密由系统处理，可能更顺）

## 5. 表盘推送

**真机验证通过（2026-08-12，Task 9）**：Band 10 Pro（M2551B1，固件 3.101.036）上完整跑通表盘安装（伊布.face，2492348 字节），手环返回 **InstallResult code=3（INSTALL_USED，已安装）**，表盘出现在手环列表。

**注意：Band 10 Pro 安装流程必须用 astrobox 的 WearPacket 协议（非 Gadgetbridge 的 Command 协议）**，见下方 WearPacket 流程。

**WearPacket 表盘安装流程（真机验证，2026-08-12）**：
1. `WearPacket{type=WATCH_FACE(4), id=PREPARE_INSTALL_WATCH_FACE(4), WatchFace{prepare_info=6{id, size, version_code=65536}}}`（加密通道）
   → 手环回 `WearPacket{type=4, id=4, WatchFace{prepare_status=5}}`（0=READY）
2. `WearPacket{type=MASS(22), id=PREPARE(0), Mass{prepare_request=1{data_type=16, data_id=md5, data_length}}}`（加密通道）
   → 手环回 `WearPacket{type=22, id=0, Mass{prepare_response=2{prepare_status=2, expected_slice_length=5}}}`（本次 12288）
3. MASS 分片上传（DATA 通道明文）：`L2[channel=2(Mass)][op=1(Write)][total u16][cur u16][fragment]`，fragment = slice_length - 6
   - MassPacket 负载：`[comp 1B=0x00][type 1B=16][md5 16B][size u32 LE][bytes] + crc32 u32 LE`
   - **批量窗口：真机验证 BATCH=2 稳定，>2 手环断连**（手环 SessionConfig TX_WIN=3）
4. 上传完成等手环推送 `WearPacket{type=WATCH_FACE(4), id=REPORT_INSTALL_RESULT(5), WatchFace{install_result=7{id, code}}}`：code 2=SUCCESS, 3=INSTALL_USED（已安装，成功）
5. 可用 `WearPacket{type=WATCH_FACE(4), id=GET_INSTALLED_LIST(0)}` 查询列表确认

**WearPacket 字段编号（关键，astrobox wear.proto）**：
- WearPacket{type=1, id=2, payload oneof}；payload 字段：Account=3, System=4, WatchFace=6, ... **Mass=24**（不是 7！）
- WatchFace payload：prepare_status=5, prepare_info=6, install_result=7, prepare_reply=9
- Mass payload：prepare_request=1, prepare_response=2
- PrepareResponse{data_id=1, prepare_status=2, expected_slice_length=5}
- PrepareStatus: READY=0, ...；InstallResult.Code: VERIFY_FAILED=0, INSTALL_FAILED=1, INSTALL_SUCCESS=2, INSTALL_USED=3

**存储查询（真机验证，2026-08-12）**：`WearPacket{type=SYSTEM(2), id=GET_STORAGE_INFO(62)}`（加密通道，payload=None）
→ 手环回 `WearPacket{type=2, id=62, System{storage_info=44{used=1, total=2}}}`。
真机值：used=12.62MB / total=259.38MB（M2551B1，含系统与表盘）。可用于安装前检查剩余空间。

- 推送 service UUID：`0000fe95-...`（V2），**真机确认**（服务存在）
- 推送 characteristic UUID：命令走 V2 TX `...5f`——**真机确认**（特征存在）；数据上传 DATA 明文通道行为已随 SPP 验证（Task 9）
- 分块大小：`expected_slice_length` 由设备在 Mass prepare_response 中给出（本次真机返回 **12288**）；fragment = slice_length - 6（L2 头 2B + total/cur 4B）。**真机确认**
- 帧格式：见第 4 节 V2 帧格式（头部/序号/数据/校验）

> 注：Gadgetbridge/Kodo 的 `Command{type=4/22}` 安装流程（XiaomiWatchfaceService / MiBand9DataUploader）是 Band 9 的 BLE 协议；Band 10 Pro 走 SPP + WearPacket（astrobox 同款），两者 type 编号巧合重叠但消息体不同，勿混用。

## 6. Bin 文件格式（表盘包解析）

（XiaomiFWHelper.parseAsWatchface，待真机确认）

- 头部 magic：fw[0]=0x5A, fw[1]=0xA5（非此即非表盘）
- id：offset 0x28 (40) 起 null-terminated ASCII 字符串，须匹配 `^\d+$`
- name：offset 0x68 (104) 起 null-terminated 字符串；若 0x68 处为 0xFFFFFFFF 则为本地化名称（i18n 表 offset 在 0x74 u32 LE、size 在 0x78 u32 LE）
- 设备类型过滤（Gadgetbridge 中按型号判断）：Band 10 Pro 的 bin 具体限制以真机验证为准

## 7. MTU / 连接

（待真机确认）

- 连接后 requestMtu(512)；maxWriteSize = mtu-3（coerceAtLeast 23）
- 表盘上传块 + 4B 头需 ≤ maxWriteSize 实际发送（发送时按 maxWriteSize 再次切分 ATT 层）

### 4.2 连接身份机制（2026-08-12 真机验证补充）

**结论：手环 BLE 连接是"身份识别制"，绑定手机后拒绝陌生设备。**

真机验证过程（Band 10 Pro，2C:0D:CF:73:D9:95，2026-08-12）：
- 状态：手环已绑定手机（未解绑），手机蓝牙关闭，手环重启后短暂广播
- 广播窗口内电脑尝试连接（dbus Connect / bluetoothctl pair+connect）：均失败
  - dbus：`le-connection-abort-by-local`（本地中止）
  - bluetoothctl：Attempting to connect 后无结果，Connected=no
  - **手环屏幕完全无反应**（无配对提示、无震动）
- 对比：昨天手环恢复出厂（清空配对）后，电脑配对成功、服务发现正常

**解释**：
- 手机上 astrobox 等工具能不解绑连接：复用手机蓝牙地址的已有配对身份（bonding），手环认识该地址 → 允许连接；连接时手环将连接从小米运动健康切换给新 App（用户观察到"配对断开"实为连接切换）
- 电脑是陌生设备：手环静默拒绝，不进入配对流程
- 因此"解绑"的本质是**清空手环侧配对身份**，让它重新接受任意设备的配对

**对工具设计的含义**：
- 电脑端 BLE 直连需要手环处于"可配对"状态（解绑/恢复出厂，或手环侧主动进入配对模式）
- **手环支持经典蓝牙 SPP 通道（RFCOMM channel 5，已确认，见 4.3 节）**，且 V2 协议实际走 SPP——正式应用应走 SPP 通道（可能绕过 BLE bonding 限制，待手环重开机后验证不解绑场景）

### 4.3 SPP 通道打通 —— V2 协议实际走经典蓝牙（2026-08-12 真机验证）

**重大结论：Band 10 Pro 的 V2 协议走经典蓝牙 SPP 通道（RFCOMM channel 5），不走 BLE 5e/5f！**

#### 发现过程（真机）

1. **SPP 服务确认**：`sdptool browse` 显示手环有 Serial Port 服务（RFCOMM channel 5, v1.02）
2. **BLE 5e/5f 写帧无响应**：即使 GATT 读写正常（读 2a00 成功）、链路加密（AUTH ENCRYPT）、MTU 517，向 5f 写 V2 帧手环始终静默 —— 通道错误
3. **SPP 连接成功**：dbus-fast + `negotiate_unix_fd=True` 注册 SPP Profile → ConnectProfile → RFCOMM fd
4. **V1 Hello 帧是必需的前置**：连接后先发 `badcfe00c00300000100ef`（V1 协议 Version 查询），手环响应 `badcfe00000600010200030131ef`（版本 01.01）确认通道
5. **V2 认证在 SPP 上正常推进**：
   - START_SESSION_REQUEST → 收到 START_SESSION_RESPONSE（type=2, opcode=2）
   - PhoneNonce（明文）→ 手环回 ACK + Data 帧（Command type=1, subtype=26 = CMD_NONCE 开头）
   - ⚠️ WatchNonce 数据收到 8B 开头 `0801101a1a021804` 后手环关机（待续：需持续累积分片 + 完整等待）

### 4.4 认证响应 `0801101a1a021804` 破解 —— WearPacket 协议 + NO_BOUND（2026-08-12 真机）

**重大发现：Band 10 Pro 的认证响应是 astrobox 的 WearPacket 协议（不是 Gadgetbridge 的 Command 协议），字节层面两者兼容。**

#### 破解过程（真机）

1. 用 astrobox 的 proto（`AstroBox-NG-Module-Pb` → `protos/xiaomi/wear.proto` + `wear_account.proto`）编译后解析手环响应 `0801101a1a021804`：
   ```
   type = 1 (ACCOUNT)
   id   = 26 (AUTH_VERIFY)
   account.error_code = 4 (NO_BOUND = 未绑定)
   ```
2. **发送帧验证**：astrobox 的 `build_auth_step_1`（WearPacket{type=ACCOUNT, id=AUTH_VERIFY, AuthAppVerify{app_random}}）与我们发送的 PhoneNonce（Command{type=1, subtype=26, Auth{phoneNonce}}）**字节完全一致**（`0801101a1a15f201120a10<nonce16>`）—— 字段编号恰好相同（type=1/id=26/payload=3 ↔ type=1/subtype=26/auth=3；auth_app_verify=30 ↔ phoneNonce=30；app_random=1 ↔ nonce=1）。
3. **结论**：认证流程本身正确，卡点在**手环绑定状态**——手环在 AUTH_VERIFY 阶段返回 `error_code=NO_BOUND(4)`，即**手环固件里没有有效的绑定记录**。

#### WearPacket 认证协议（astrobox 同款，Band 10 Pro）

| 阶段 | 消息 | 方向 | 内容 |
|---|---|---|---|
| 1 | `WearPacket{type=ACCOUNT(1), id=AUTH_VERIFY(26), AuthAppVerify{app_random}}` | 电脑→手环 | 等价于 PhoneNonce |
| 2 | `WearPacket{type=ACCOUNT, id=AUTH_VERIFY(26), AuthDeviceVerify{device_random, device_sign}}` | 手环→电脑 | 等价于 WatchNonce{nonce, hmac} |
| 3 | `WearPacket{type=ACCOUNT, id=AUTH_CONFIRM(27), AuthAppConfirm{app_sign, encrypt_companion_device}}` | 电脑→手环 | 等价于 AuthStep3 |
| 4 | `WearPacket{type=ACCOUNT, id=AUTH_CONFIRM(27), AuthDeviceConfirm{confirm_result}}` | 手环→电脑 | 认证完成 |
| 错误 | `WearPacket{type=ACCOUNT, id=AUTH_VERIFY(26), error_code}` | 手环→电脑 | `NO_BOUND(4)=未绑定`，`HAVE_BOUND(1)=已绑定` 等 |

加密算法与 Gadgetbridge/Kodo 相同（kdf_miwear、verifyWatchHmac、AES-CCM encrypt_companion_device）。

#### 绑定状态要求（关键约束）

- **AUTH_VERIFY 认证要求手环固件里有绑定记录**：手环恢复出厂 / App 内解绑会清除绑定 → 返回 NO_BOUND。
- **正确流程**（Gadgetbridge issue #6486 用户验证）：
  1. 官方 App（小米运动健康）绑定手环 → 从手机日志 `XiaomiFit.main.log` 提取 token（authkey）
  2. 蓝牙设置中 unpair 手环（**不要恢复出厂**）
  3. 手环上选择"配对新手环"
  4. 电脑端用 authkey 认证 → 成功
- **unpair（蓝牙解配）≠ 恢复出厂/App 内解绑**：只有前者能保留手环固件绑定记录（authkey 继续有效）。
- 手环重新绑定后需在**手环端取消手机配对**，让电脑能重新连接（BLE 配对/SPP）。

### 4.5 真机认证验证通过（2026-08-12）—— Task 7 完成

**SPP 认证握手在 Band 10 Pro 真机上完整跑通（authkey 绑定状态下）！**

完整交互日志（成功路径）：
```
→ V1 Hello badcfe00c00300000100ef
→ START_SESSION_REQUEST (seq=0)
RX type=2 seq=0 payload=02010300030131020200008003020003000402007017   ← START_SESSION_RESPONSE
→ PhoneNonce <16B>
RX type=1 seq=0  ← ACK
RX type=3 seq=0 payload=0101 + 0801101a1a37fa01340a10<nonce16>1220<hmac32>  ← WatchNonce（61B）
→ ACK seq=0
watch HMAC 验证通过
→ AuthStep3 (seq=1)
RX type=1 seq=1  ← ACK
RX type=3 seq=1 payload=0101 + 0801101b1a138a0210080110bff69fbf0f18ca8fa240208441  ← subtype=27 确认
★★★ 认证成功（subtype=27）★★★
```

#### 成功的前提条件（真机验证）

1. **手环必须处于已绑定状态**（authkey 在手环固件中有有效绑定记录）：
   - 用户用官方 App 重新绑定手环 → 提取新 authkey `22dc81dea345e53c4d8c8a96fecc7454`
   - 手环恢复出厂/解绑后 authkey 失效 → AUTH_VERIFY 返回 `error_code=NO_BOUND(4)`
2. **电脑端 BLE 配对必须用 DisplayYesNo agent**（非 NoInputNoOutput）：
   - 手环在配对模式发起的是需要确认的配对（手环屏幕显示"请在手机上确认配对"）
   - `NoInputNoOutput` agent 无法响应 → 手环显示"配对失败"
   - `DisplayYesNo` agent（`RequestConfirmation` 自动接受）→ 配对成功
3. **配对成功后**：Paired=True + Connected=True → SPP ConnectProfile 成功 → V1 Hello → V2 认证。
4. **认证成功后**：后续命令可走 PROTOBUF 加密通道（encrypt=true，encryptV2/decryptV2）。

#### 数据验证

- WatchNonce 完整 61B：`0801101a1a37fa01340a10<nonce16>1220<hmac32>` —— 与 golden `_G_WATCH_NONCE_CMD` 结构一致（`1a 37` 表示 auth 55B，field 31 wire2）
- watch HMAC 用 deriveSession 派生的 decKey 验证通过（authkey 正确）
- AuthStep3 后手环返回 `type=1, subtype=27`（CMD_AUTH）→ 认证完成
- 认证期间所有数据帧为明文（opCode=1）；认证完成后 PROTOBUF 通道切换为加密（opCode=2）

#### 关键技术点

| 项 | 值 | 说明 |
|---|---|---|
| SPP UUID | `00001101-0000-1000-8000-00805f9b34fb` | 标准 Serial Port |
| RFCOMM channel | 5 | 从 SDP 查询获得 |
| **dbus FD 支持** | `MessageBus(negotiate_unix_fd=True)` | **必须**！否则 bluetoothd 报 "Tried to send message with Unix file descriptors to a client that doesn't support that" |
| **dbus 库选择** | **dbus-fast**（非 dbus-next） | dbus-next 的 Profile 方法签名 `oha{sv}` 无法正确暴露（UnknownMethod 错误） |
| V1 Hello 帧 | `badcfe00c00300000100ef` | V1 版本协商：channel=0(Version), needsResponse, OPCODE_READ |
| V1 响应 | `badcfe00000600010200030131ef` | 版本 01.01 → 确认后进入 V2 |

#### 对工具设计的含义（重大修正）

- **正式应用应走 SPP 通道**（不是 BLE GATT！）—— 这解释了为什么 BLE 上认证一直无响应
- SPP 通道**可能绕过 BLE bonding 限制**（SPP 配对独立于 BLE bonding）—— 4.2 节的"待确认"已变为"很可能可行"：若 SPP 连接无需解绑，电脑端工具也能达到 astrobox 的"不解绑"体验（待手环重新开机后验证）
- AstroBox 在 Linux 用 bluer（Rust）实现同样的 SPP Profile 连接，与我们验证的 dbus-fast 路径等价
