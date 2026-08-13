# 小米手环 10 Pro 表盘直装 —— 研究总结

> 阶段 1（POC 协议验证）完成，2026-08-12。
> 技术细节见 `protocol-notes.md`；本文件为研究历程与结论的人读总结。

## 1. 项目目标

通过蓝牙直连小米手环 10 Pro（**不经官方 App**），将 `.bin`/`.face` 表盘文件安装到手环。
阶段 1 用 Python POC 验证协议可行性；阶段 2 将构建 Tauri 跨平台 GUI。

## 2. 研究历程（时间线）

### 8 月 11 日：BLE 路径探索

- GATT 枚举确认：手环只有 V2 特征（fe95 服务下 `5e/5f`），无 V1 特征（`51/52/53/55`）
- 与 Band 9 参考实现（Gadgetbridge / Kodo）协议匹配
- **发现 BLE 认证无响应的迹象**：即使 ATT 读写正常，向 V2 TX 写帧手环始终静默

### 8 月 12 日上午：SPP 通道突破

- **重大发现：V2 协议实际走经典蓝牙 SPP 通道（RFCOMM channel 5），不走 BLE GATT！**
  - `sdptool browse` 确认手环有 Serial Port 服务
  - dbus-fast + `negotiate_unix_fd=True` 注册 Profile → ConnectProfile 成功
- V1 Hello 帧是必需前置（`badcfe00c00300000100ef` → 版本响应确认协议版本）
- START_SESSION → PhoneNonce → WatchNonce 流程在 SPP 上正常推进，但卡在 WatchNonce 响应异常（`0801101a1a021804`）

### 8 月 12 日下午：WearPacket 协议破解 + 认证/安装全通
- **破解 `0801101a1a021804`**：用 astrobox 的 proto 编译解析，确认它是 **WearPacket 协议**的 `error_code=NO_BOUND（未绑定）`——不是 Gadgetbridge 的 Command 协议！
- **发现手环绑定状态是认证前提**：手环恢复出厂/解绑 → 认证返回 NO_BOUND；重新绑定并提取新 authkey 后认证成功
- **配对方式修正**：手环配对需 **DisplayYesNo agent**（NoInputNoOutput 无法响应"请在手机上确认配对"）
- **SPP 认证完整跑通**：PhoneNonce → WatchNonce(61B) → HMAC 验证 → AuthStep3 → subtype=27
- **表盘安装完整跑通**：WatchFace PREPARE → Mass PREPARE → MASS 分片上传 → **InstallResult code=3（已安装）**，表盘出现在手环列表

### 8 月 13 日：Task 16 收尾 + Linux UI 修复 + Android 版移植

- **Task 16 真机验收全项通过**（扫描/认证/安装/错误路径/中断）；修复 4 个真机 bug：ConnectProfile 错误误判、V2Accumulator 双重消费、WearPacket Mass type=22、MASS 必须等 ACK
- **安装确认可靠性调查**：POC 与 Rust 发送字节逐帧一致，但手环推 InstallResult（id=5）不可靠（随时间/存储状态变化，POC 也失败）→ 采用「InstallResult 快速 + GET_INSTALLED_LIST 兜底」双通道；同时修复上传后 seq 未更新 bug
- **Tauri 白屏根因**：Node 17+ localhost→IPv6 `::1`，Vite 只监听 `::1`，WebKitGTK 用 IPv4 连接失败 → 强制 `vite host=127.0.0.1`
- **UI 改进**：安装后留在安装页（保持认证状态）+ 文件选择 + 存储显示
- **Android 版移植**（详见 `android-port-notes.md`）：传输抽象层（tokio trait）→ 平台条件编译 → Kotlin RFCOMM + JNI fd 桥 → 蓝牙扫描 → APK 构建成功。核心结论：**协议层纯 Rust 可 100% 复用，仅连接层需按平台重写**

## 3. 关键技术发现（按重要性）

### 3.1 Band 10 Pro 使用 astrobox 的 WearPacket 协议

- 认证帧与 Gadgetbridge 的 `Command` 协议**字节兼容**（字段编号巧合重叠：type=1/id=26/payload=3 ↔ type=1/subtype=26/auth=3），但**消息语义不同**
- **表盘安装流程必须用 WearPacket**（`WearPacket{type=..., id=..., payload=...}`），不能套用 Gadgetbridge 的 `Command{type=4/22}`
- WearPacket payload 字段编号（关键）：**Mass=24**（不是 7！），WatchFace=6，Account=3，System=4

### 3.2 通道选择：SPP（经典蓝牙），不是 BLE

- V2 协议全部走 RFCOMM channel 5
- BLE 5e/5f 特征写帧无响应（通道错误）
- SPP 通道可绕过 BLE bonding 限制（电脑端无需手环解绑即可连接认证）

### 3.3 认证前提：手环必须"已绑定"

- AUTH_VERIFY 需要手环固件里有有效绑定记录（authkey 匹配）
- 手环恢复出厂 / App 内解绑 → 绑定清除 → 认证返回 `error_code=NO_BOUND(4)`
- 正确流程：官方 App 绑定 → 提取 authkey → **蓝牙 unpair（不恢复出厂）** → 手环选"配对新手环" → 电脑端认证

### 3.4 配对方式：DisplayYesNo agent

- 手环配对模式发起的是需确认的配对（屏幕显示"请在手机上确认配对"）
- `NoInputNoOutput` agent 无法响应 → 手环显示"配对失败"
- `DisplayYesNo` agent（`RequestConfirmation` 自动接受）→ 配对成功

### 3.5 MASS 分片上传细节

- 帧格式：`L2[channel=2(Mass)][op=1(Write)][total u16][cur u16(1 起)][fragment]`
- fragment = `expected_slice_length - 6`（L2 头 2B + total/cur 4B）
- MassPacket 负载：`[comp 0x00][type=16][md5 16B][size u32 LE][bytes] + crc32 u32 LE`
- **批量窗口 BATCH=2 稳定**（手环 SessionConfig TX_WIN=3；BATCH>2 时手环断连）
- 上传完成后**等手环主动推送 InstallResult**（非主动查询）

### 3.6 辅助命令（真机验证）

- 存储查询：`WearPacket{type=SYSTEM(2), id=GET_STORAGE_INFO(62)}` → `StorageInfo{used, total}`（真机：used=12.62MB / total=259.38MB）
- 表盘列表：`WearPacket{type=WATCH_FACE(4), id=GET_INSTALLED_LIST(0)}`

## 4. 最终可用流程（真机验证通过）

```
1. 连接 SPP（RFCOMM ch5，dbus-fast + negotiate_unix_fd）
2. V1 Hello 版本协商
3. V2 START_SESSION_REQUEST → START_SESSION_RESPONSE
4. authkey 认证：PhoneNonce → WatchNonce → HMAC 验证 → AuthStep3 → subtype=27
5. WearPacket WatchFace PREPARE_INSTALL → prepare_status=0
6. WearPacket Mass PREPARE → prepare_response（expected_slice_length=12288）
7. MASS 分片上传（BATCH=2，逐批等 ACK）
8. 等手环推送 InstallResult（code 2=SUCCESS / 3=INSTALL_USED）
9. 完成，表盘出现在手环列表
```

## 5. 关键坑与解决方案

| 坑 | 症状 | 解决 |
|---|---|---|
| 通道错误 | BLE 写帧无响应 | 改走 SPP（RFCOMM ch5） |
| 协议识别错误 | 响应解析为"Auth field3=4" | 用 astrobox WearPacket 解析 = NO_BOUND |
| 手环未绑定 | 认证 NO_BOUND | 官方 App 重新绑定 + 提取新 authkey |
| 配对失败 | 手环"配对失败" | DisplayYesNo agent 自动接受确认 |
| 批量过大断连 | 上传中途蓝牙断开 | BATCH=2 |
| 上传后无反应 | 数据传完表盘不出现 | 等手环推送 InstallResult（不能只看发送成功） |
| Mass 字段号错 | prepare 无响应 | Mass=24（非 7） |

## 6. 阶段 1 验收结论

- ✅ 认证握手成功（authkey）
- ✅ 表盘推送完成，手环实际显示新表盘
- ✅ 协议差异笔记完整（第 3/4/5 节均含真机数据）
- **阶段 1 完成** → 进入阶段 2（Tauri 应用）

## 7. 遗留问题与下一步

- **POC 脚本**：`pocs/{common,scan,auth,spp_fast,install}.py` 可用；`install.py` 含完整安装流程
- **阶段 2（Linux GUI）**：Tauri 2.x 应用已完成真机验证（Task 16 全项通过，2026-08-13）；安装确认用「InstallResult + 列表查询」双通道
- **阶段 3（Android 版）**：进行中（2026-08-13）——传输抽象层已重构，APK 可构建；待 Android 真机联调 + SAF 文件选择，详见 `android-port-notes.md`
- **协议常量**集中于 Rust `consts.rs`，值取自协议笔记
- **注意**：authkey 与手环绑定状态关联；重新绑定后需重新提取；手环反复安装会累积存储（覆盖不彻底），建议定期用官方 App 清理
