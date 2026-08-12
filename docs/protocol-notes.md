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
| 固件版本 | 待补充（可从 Device Information 服务 180a 读取，后续补充） |
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

- 推送 service UUID：`0000fe95-...`（V2），**真机确认**（服务存在）
- 推送 characteristic UUID：命令走 V2 TX `...5f`——**真机确认**（特征存在，write-without-response）；数据上传 DATA 明文通道行为待 Task 8/9 验证
- 分块大小：`chunkSize` 由设备在 uploadAck 中给出（无则默认 2048）；每块 partSize = chunkSize - 4（至少 64）；ATT 层实际发送再按 maxWriteSize = mtu-3 切分。待真机确认
- 帧格式：见第 4 节 V2 帧格式（头部/序号/数据/校验）

**安装流程**（XiaomiWatchfaceService / HybridWatchface，待真机确认）：
1. 发 `Command{type=4, subtype=4(CMD_WATCHFACE_INSTALL), Watchface.watchfaceInstallStart{id, size}}`（加密）
2. 收 `Command{type=4, subtype=4, Watchface.installStatus}`；installStatus==0 才继续（2=已安装）
3. 数据上传（DATA 明文通道，见下）
4. 发 `Command{type=4, subtype=1(CMD_WATCHFACE_SET), Watchface.watchfaceId=id}` 激活

**数据上传**（MiBand9DataUploader / XiaomiDataUploadService，待真机确认）：
1. 发 `Command{type=22, subtype=0, DataUpload.dataUploadRequest{type=16(TYPE_WATCHFACE), md5sum=MD5(bytes), size}}`（加密）
2. 收 `Command{type=22, subtype=0, DataUpload.dataUploadAck{unknown2（须为0）, resumePosition, chunkSize（无则默认2048）}}`
3. 构造带帧数据：
   ```
   framed = [0x00][type u8][md5 16B][size u32 LE][bytes]
   withCrc = framed + [crc32 u32 LE of framed]   # java.util.zip.CRC32
   ```
   分块：partSize = (chunkSize - 4)（至少 64）；每块 = [totalParts u16 LE][current u16 LE][data]
   通过 DATA 通道（明文）逐块发送，块大小上限 maxWriteSize = mtu-3

- 应答格式：`Command{type=22, subtype=0, DataUpload.dataUploadAck}`，见上；上传完成由安装流程的 installStatus 应答确认。待真机确认

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
- 若手环支持经典蓝牙 SPP 通道（待确认），SPP 可能绕过该限制（SPP 配对独立于 BLE bonding）——当前 Band 10 Pro BR/EDR 扫描未发现 SPP 服务，待进一步验证
