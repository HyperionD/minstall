# 手环 10 Pro BLE 协议笔记

> 本文件是 POC 阶段的核心产出。所有协议值必须来自真机验证或明确标注来源的参考实现，禁止臆造。
>
> **标注约定**：标有 `待真机确认` 的值来自参考实现（第 1 节的 Gadgetbridge / Kodo，基于 Band 9 / Band 9 Active），尚未经 Band 10 Pro 真机验证。真机验证后应移除此标注并改为实测值。

## 1. 参考实现来源

获取日期：2026-08-11。参考设备型号：Xiaomi Smart Band 9 / Band 9 Active（与 Band 10 Pro 同属小米手环系，协议版本 V2，预计高度相似但**待真机确认**）。

| 项目 | 仓库 | 关键文件 | 许可 | 说明 |
|---|---|---|---|---|
| Gadgetbridge | https://github.com/Freeyourgadget/Gadgetbridge | `app/src/main/java/nodomain/freeyourgadget/gadgetbridge/service/devices/xiaomi/services/XiaomiWatchfaceService.java`（表盘安装命令）、`.../xiaomi/services/XiaomiDataUploadService.java`（数据上传）、`.../xiaomi/devices/xiaomi/XiaomiFWHelper.java`（bin 解析）、`app/src/main/proto/xiaomi.proto`（protobuf 定义） | AGPL-3.0 | 支持 miband8 / miband8active / miband8pro / miband9 / miband9pro / redmiwatch3active 等 |
| Kodo | https://github.com/kidneyweakx/Kodo | `android-port/src/main/java/com/kidneyweakx/miband9active/xiaomi/protocol/XiaomiUuids.kt`、`.../protocol/XiaomiSppPacketV2.kt`、`.../protocol/MiBand9BleDriver.kt`、`.../auth/XiaomiAuthSession.kt` + `.../auth/XiaomiCrypto.kt`、`.../services/MiBand9DataUploader.kt`、`android-port/src/main/proto/xiaomi.proto` | AGPL-3.0 | Kotlin port，聚焦 Band 9 Active |

> 注意：AGPL-3.0 许可的参考实现代码不可直接复制进本项目；仅作协议参考。

## 2. 设备信息（真机）
- 固件版本：
- 表盘分辨率：

## 3. GATT 服务枚举结果（真机，scan.py 产出）

下表 UUID 来自参考实现（Kodo `XiaomiUuids.kt`），供 scan.py 枚举时对照；真机枚举结果待 POC 验证。服务/特征归属（V1 vs V2）亦待真机确认。

| Service UUID | Characteristic UUID | Properties | 疑似用途（对照参考实现） |
|---|---|---|---|
| `0000fe95-0000-1000-8000-00805f9b34fb`（SERVICE_V2） | `0000005e-0000-1000-8000-00805f9b34fb`（V2 RX） | notify | V2 通道接收（读方向），待真机确认 |
| 同上 | `0000005f-0000-1000-8000-00805f9b34fb`（V2 TX） | write | V2 通道发送（写方向），待真机确认 |
| 同上 | `00002902-0000-1000-8000-00805f9b34fb`（CCC） | — | 特征配置描述符（enable notification），待真机确认 |
| `0000fe95-0000-1000-8000-00805f9b34fb`（V1 同 service） | `00000051-0000-1000-8000-00805f9b34fb`（COMMAND_READ）、`00000052-0000-1000-8000-00805f9b34fb`（COMMAND_WRITE）、`00000053-0000-1000-8000-00805f9b34fb`（ACTIVITY_DATA）、`00000055-0000-1000-8000-00805f9b34fb`（DATA_UPLOAD） | — | Band 8 时代的 V1 特征；V2 使用 5e/5f，待真机确认 |

## 4. authkey 认证

- authkey 长度（hex 字符数）：32（16 字节）；"0x" 前缀可接受，待真机确认
- 认证 service UUID：`0000fe95-0000-1000-8000-00805f9b34fb`，待真机确认
- 认证 characteristic UUID（写/读/通知）：V2 TX `...5f`（写）/ V2 RX `...5e`（通知），待真机确认
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

## 5. 表盘推送

- 推送 service UUID：`0000fe95-...`（V2），待真机确认
- 推送 characteristic UUID：命令走 V2 TX `...5f`；数据上传走 DATA 明文通道，待真机确认
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
