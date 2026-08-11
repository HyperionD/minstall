# 小米手环 10 Pro 表盘直装工具 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 通过 BLE 直连小米手环 10 Pro，将 `.bin` 表盘文件直接安装到手环（不经过官方 App），以跨平台 Tauri GUI 形式交付。

**Architecture:** 两阶段。阶段 1 用 Python + bleak 写 POC 验证脚本，以最便宜的方式验证 Band 10 Pro 与社区 Band 9（Furhat 族）BLE 协议的兼容性，产出 `docs/protocol-notes.md` 协议笔记。阶段 2 用 Tauri 2.x（Rust `btleplug` + React/TS 前端）搭建正式应用，协议常量集中存放于 `consts.rs`，从协议笔记填充。

**Tech Stack:** Python 3.11+ / bleak（POC）；Rust / Tauri 2.x / btleplug（正式）；React + TypeScript（前端）。

## Global Constraints

- 目标设备：小米手环 10 Pro；范围仅"安装表盘"，不做预览/设备表盘管理/表盘商店/authkey 提取/多设备并发。
- 认证方式：用户手动输入或从文件加载 authkey（hex 字符串），提取由用户自行完成。
- 使用工具前必须断开手机与手环的 BLE 连接（BLE 独占），所有界面需有醒目提示。
- 协议常量一律集中在 `consts.rs`（Rust）与 `protocol-notes.md`（POC），不散落硬编码。
- 所有错误显式处理，不静默失败；错误信息对用户可操作。
- Git：Conventional Commits；功能开发在 `feat/watchface-installer` 分支上进行，不在 main 上改代码。
- 协议数据真实性优先：凡涉及协议 UUID/帧格式/密钥长度的值，必须来自 `docs/protocol-notes.md`（其内容由 Task 3 获取、Task 5/7/9 真机验证填充），不得臆造。

---

## 文件结构

```
minstall/
├── pocs/                          # 阶段 1
│   ├── requirements.txt           # bleak 等依赖
│   ├── common.py                  # 日志输出、JSON 结果格式化（scan/auth/install 共用）
│   ├── scan.py                    # 扫描 + GATT 枚举 → JSON
│   ├── auth.py                    # authkey 认证握手
│   └── install.py                 # bin 读取 + 分块推送
├── docs/protocol-notes.md         # 协议笔记：参考值 + 真机验证结果
├── src-tauri/src/
│   ├── main.rs                    # 入口
│   ├── commands.rs                # Tauri command 桥接
│   ├── events.rs                  # 进度事件发射
│   ├── ble/{mod,scanner,connection,errors}.rs
│   └── protocol/{mod,auth,watchface,consts}.rs
├── src/                           # React 前端（Tauri 模板）
└── README.md
```

---

### Task 1: 项目骨架 + 分支

**Files:**
- Create: `.gitignore`（已存在，确认内容）、`README.md`、`docs/protocol-notes.md`

**Interfaces:**
- Produces: 工作分支 `feat/watchface-installer`；`docs/protocol-notes.md` 模板（含"参考值待填充"表格）；README 项目简介。

- [ ] **Step 1: 创建功能分支**

```bash
cd /home/hyperion/projects/minstall
git checkout -b feat/watchface-installer
```

- [ ] **Step 2: 写 README**

```markdown
# minstall

小米手环 10 Pro 表盘直装工具（BLE 直连安装 .bin 表盘，不经过官方 App）。

- 阶段 1：POC 协议验证（`pocs/`，Python + bleak）
- 阶段 2：Tauri 跨平台 GUI

使用前提：已通过第三方工具获取手环 authkey；使用前断开手机与手环的连接。
```

- [ ] **Step 3: 创建协议笔记模板 `docs/protocol-notes.md`**

```markdown
# 手环 10 Pro BLE 协议笔记

> 本文件是 POC 阶段的核心产出。所有协议值必须来自真机验证或明确标注来源的参考实现，禁止臆造。

## 1. 参考实现来源
（Task 3 填充：仓库地址、获取日期、参考的设备型号）

## 2. 设备信息（真机）
- 固件版本：
- 表盘分辨率：

## 3. GATT 服务枚举结果（真机，scan.py 产出）
| Service UUID | Characteristic UUID | Properties | 疑似用途（对照参考实现） |
|---|---|---|---|

## 4. authkey 认证
- authkey 长度（hex 字符数）：
- 认证 service UUID：
- 认证 characteristic UUID（写/读/通知）：
- 握手流程（步骤序列 + 每步帧格式）：

## 5. 表盘推送
- 推送 service UUID：
- 推送 characteristic UUID：
- 分块大小：
- 帧格式（头部/序号/数据/校验）：
- 应答格式：
```

- [ ] **Step 4: 提交**

```bash
git add README.md docs/protocol-notes.md
git commit -m "chore: scaffold project and protocol notes template"
```

---

### Task 2: POC 环境

**Files:**
- Create: `pocs/requirements.txt`、`pocs/common.py`

**Interfaces:**
- Produces: `common.py` 提供 `log(msg)`（带时间戳打印到 stderr）与 `emit_json(obj)`（JSON 序列化打印到 stdout，供脚本链式调用）。

- [ ] **Step 1: 写 `pocs/requirements.txt`**

```
bleak>=0.22.0
```

- [ ] **Step 2: 写 `pocs/common.py`**

```python
"""POC 脚本公共工具：日志走 stderr，结构化结果走 stdout。"""
import json
import sys
import time


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


def emit_json(obj) -> None:
    """向 stdout 输出一行 JSON，供脚本链式解析。"""
    print(json.dumps(obj, ensure_ascii=False), flush=True)
```

- [ ] **Step 3: 验证导入正常**

Run: `cd pocs && python -c "import common; common.log('ok'); common.emit_json({'a': 1})"`
Expected: stderr 出现日志行，stdout 出现 `{"a": 1}`

- [ ] **Step 4: 提交**

```bash
git add pocs/requirements.txt pocs/common.py
git commit -m "chore(pocs): add environment and common utils"
```

---

### Task 3: 获取 Band 9 参考协议资料

**Files:**
- Modify: `docs/protocol-notes.md` 第 1 节

**Interfaces:**
- Produces: `docs/protocol-notes.md` 第 1 节填入参考实现来源；第 3/4/5 节的"参考列"填入可对照的值（未经验证的值标注"待真机确认"）。

- [ ] **Step 1: 定位参考实现**

优先按以下途径获取 Band 9（或 Band 8）开源的 BLE 表盘安装/协议实现：

1. 询问用户是否有已知的社区项目（BandInstaller / miband 系列 / Furhat 协议相关仓库），若有则直接使用其 URL
2. 网络可用时：GitHub 搜索 `band 9 watchface install`、`xiaomi band ble watchface`、`miband authkey`，克隆候选仓库
3. 若以上均不可行：记录"参考值不可得"，POC 退化为纯探索（scan.py 枚举全部服务，凭服务特征推断用途），并在协议笔记中如实标注

把结果填入 `docs/protocol-notes.md` 第 1 节（仓库地址、获取日期、参考设备型号），并把第 3/4/5 节中能对照的值写入对应列，标注来源与置信度。

- [ ] **Step 2: 提交**

```bash
git add docs/protocol-notes.md
git commit -m "docs(protocol): record reference implementation source"
```

---

### Task 4: scan.py —— GATT 枚举工具

**Files:**
- Create: `pocs/scan.py`
- Test: 纯逻辑内联自检（`python pocs/scan.py --self-test`）

**Interfaces:**
- Produces: CLI `python pocs/scan.py`（交互式：选设备）与 `python pocs/scan.py --address <addr>`；stdout 输出 `{device, services:{uuid: [{uuid, properties}]}}` JSON。`filter_relevant(devices)` 与 `services_to_json(services)` 两个纯函数供测试。

- [ ] **Step 1: 写 scan.py（含纯函数 + 自检）**

```python
"""BLE 扫描 + GATT 枚举。不依赖预设协议 UUID —— 枚举结果是协议笔记的数据源。"""
import asyncio
import sys

from bleak import BleakClient, BleakScanner

from common import emit_json, log

RELEVANT_KEYWORDS = ("mi", "band", "xiaomi")


def filter_relevant(devices):
    """过滤出手环相关设备（纯函数，可测试）。"""
    out = []
    for d in devices:
        name = (d.name or "").lower()
        if any(k in name for k in RELEVANT_KEYWORDS):
            out.append({"name": d.name, "address": d.address, "rssi": getattr(d, "rssi", None)})
    return out


def services_to_json(services):
    """把 bleak 服务树转成 JSON 结构（纯函数，可测试）。"""
    result = {}
    for service in services:
        result[str(service.uuid)] = [
            {
                "uuid": str(c.uuid),
                "properties": sorted(p.name for p in c.properties),
            }
            for c in service.characteristics
        ]
    return result


async def scan_devices(timeout: float = 10.0):
    devices = await BleakScanner.discover(timeout=timeout)
    relevant = filter_relevant(devices)
    log(f"发现 {len(relevant)} 个相关设备")
    return relevant


async def dump_gatt(address: str):
    async with BleakClient(address) as client:
        await client.get_services()
        return {"device": address, "services": services_to_json(client.services)}


async def main():
    args = sys.argv[1:]
    if "--address" in args:
        addr = args[args.index("--address") + 1]
        emit_json(await dump_gatt(addr))
        return
    if "--self-test" in args:
        # 纯函数自检：构造假数据验证过滤与序列化
        class FakeDev:
            name = "Xiaomi Band 10 Pro"
            address = "AA:BB:CC:DD:EE:FF"
            rssi = -55
        fake = FakeDev()
        filtered = filter_relevant([fake, type("F", (), {"name": "iPhone", "address": "X", "rssi": 0})()])
        assert filtered[0]["name"] == "Xiaomi Band 10 Pro", filtered
        assert len(filtered) == 1, filtered
        print("self-test OK")
        return
    # 交互模式：扫描 → 列出 → 选号枚举 GATT
    devices = await scan_devices()
    for i, d in enumerate(devices):
        print(f"[{i}] {d['name']}  {d['address']}  rssi={d['rssi']}")
    if not devices:
        sys.exit("未发现设备，请确认手环已开启蓝牙且未被手机连接")
    choice = int(input("选择设备编号: "))
    emit_json(await dump_gatt(devices[choice]["address"]))


if __name__ == "__main__":
    asyncio.run(main())
```

- [ ] **Step 2: 运行自检**

Run: `cd pocs && python scan.py --self-test`
Expected: 输出 `self-test OK`

- [ ] **Step 3: 提交**

```bash
git add pocs/scan.py
git commit -m "feat(pocs): add BLE scan and GATT dump tool"
```

---

### Task 5: 真机扫描验证（需用户配合）

**Files:**
- Modify: `docs/protocol-notes.md` 第 2/3 节

**Interfaces:**
- Consumes: Task 4 的 `scan.py`
- Produces: 协议笔记中的设备信息与 GATT 枚举结果（含与参考实现的对照）；为 Task 6 的认证特征定位提供依据。

- [ ] **Step 1: 用户准备设备**

用户确认：手环蓝牙开启、已从手机端断开（App 内解绑或关闭手机蓝牙）。

- [ ] **Step 2: 运行扫描**

Run: `cd pocs && python scan.py`
Expected: 列出 `Xiaomi Band 10 Pro`（或类似名称）及其地址。

- [ ] **Step 3: 枚举 GATT 并记录**

选择设备编号，将 stdout 的 JSON 保存到 `docs/protocol-notes.md` 第 3 节表格（按 `uuid/properties` 转录）。
同时记录第 2 节设备信息：从手环设置页读固件版本、查产品规格确认表盘分辨率（如 432×480，以真机规格为准）。

- [ ] **Step 4: 对照参考实现，标注疑似用途**

将 Task 3 参考实现中出现的 UUID 与枚举结果匹配，在第 3 节"疑似用途"列标注（如 auth / watchface upload / firmware）。
若完全无匹配，在笔记中明确记录"协议与参考实现不同，需逆向（超出当前范围）"，并在本任务结束时报给用户决策。

- [ ] **Step 5: 提交**

```bash
git add docs/protocol-notes.md
git commit -m "docs(protocol): record real device GATT enumeration"
```

---

### Task 6: auth.py —— 认证握手

**Files:**
- Create: `pocs/auth.py`
- Test: 握手帧构造纯函数自检

**Interfaces:**
- Consumes: `docs/protocol-notes.md` 第 4 节（认证 UUID 与流程，Task 5 后应可定位）；`common.py`
- Produces: CLI `python pocs/auth.py --address <addr> --authkey <hex>`；stdout 输出 `{authenticated: bool, detail}`；纯函数 `build_auth_frames(authkey_hex, flow)` 返回帧字节列表，`parse_auth_response(data, flow)` 返回 `(ok, detail)`。

- [ ] **Step 1: 确认认证流程数据可用**

阅读 `docs/protocol-notes.md` 第 4 节。若流程值已从参考实现+真机对照确定，按第 2-4 步实现；若第 4 节仍为空（参考值不可得且真机无法推断），**停止并报告**：认证流程无法凭空实现，需回到 Task 3 补资料或与用户确认逆向方案，不写猜测代码。

- [ ] **Step 2: 写 auth.py（帧构造/解析为纯函数，BLE 交互薄封装）**

```python
"""authkey 认证握手。帧格式与 UUID 来自 docs/protocol-notes.md 第 4 节。"""
import asyncio
import sys

from bleak import BleakClient

from common import emit_json, log

# ---- 以下值必须来自 protocol-notes.md 第 4 节，禁止臆造 ----
AUTH_SERVICE_UUID = None      # 认证 service，笔记填充后替换
AUTH_WRITE_CHAR = None        # 写特征
AUTH_NOTIFY_CHAR = None       # 通知特征
AUTH_FLOW = []                # 握手步骤：如 [("send", 0x01), ("expect", 0x10)]，每步的帧字节在笔记中定义
AUTHKEY_LEN = 0               # authkey hex 字符数
# ------------------------------------------------------------------


def build_auth_frames(authkey_hex: str, flow: list):
    """按 flow 生成握手帧序列（纯函数）。flow 每项为 (step_name, payload_or_template)。
    具体帧拼接规则以协议笔记第 4 节为准；此处仅给出结构骨架。"""
    frames = []
    for step in flow:
        name, payload = step
        if name == "send":
            frames.append(bytes(payload))
        else:
            frames.append(None)  # 等待应答，由 parse_auth_response 处理
    return frames


def parse_auth_response(data: bytes, flow: list):
    """解析握手应答（纯函数）。返回 (ok: bool, detail: str)。"""
    # 以协议笔记中的应答格式实现；未知应答返回 (False, f"unexpected: {data.hex()}")
    return (False, f"unexpected response: {data.hex()}")


async def authenticate(address: str, authkey_hex: str):
    if len(authkey_hex) != AUTHKEY_LEN:
        return {"authenticated": False, "detail": f"authkey 长度应为 {AUTHKEY_LEN} 字符"}
    log(f"连接 {address} ...")
    async with BleakClient(address) as client:
        # 按 AUTH_FLOW 逐帧执行：build_auth_frames 构造 → 写入 → 订阅通知收应答 → parse_auth_response
        # 具体循环按协议笔记第 4 节流程实现
        return {"authenticated": True, "detail": "流程占位：请按协议笔记第 4 节实现握手循环"}
```

（说明：`AUTH_FLOW`/`parse_auth_response` 的真实实现依赖协议笔记的具体帧格式。实现者按笔记填充上述标 `None`/占位处；若笔记缺失该数据则执行 Step 1 的停止规则。）

- [ ] **Step 3: 自检帧构造函数**

Run: `cd pocs && python -c "from auth import build_auth_frames; print(build_auth_frames('00'*32, [('send',[0x01,0x02])]))"`
Expected: 输出 `[b'\x01\x02']`（在填充 AUTH_FLOW 后，按笔记实际值自检）

- [ ] **Step 4: 提交**

```bash
git add pocs/auth.py
git commit -m "feat(pocs): add authkey handshake"
```

---

### Task 7: 真机认证验证（需用户配合）

**Files:**
- Modify: `docs/protocol-notes.md` 第 4 节

**Interfaces:**
- Consumes: Task 6 `auth.py`
- Produces: 第 4 节标注"真机验证通过/失败"；失败时记录实际应答字节，供修正帧格式。

- [ ] **Step 1: 用户提供 authkey**

用户用第三方工具提取手环 authkey 后，以 hex 字符串提供（测试阶段可临时写入环境变量 `MINSTALL_AUTHKEY`）。

- [ ] **Step 2: 运行认证**

Run: `cd pocs && python auth.py --address <addr> --authkey "$MINSTALL_AUTHKEY"`
Expected: `{"authenticated": true, ...}`；若失败，将日志与应答字节记入协议笔记第 4 节，回到 Task 6 修正帧格式后重试。

- [ ] **Step 3: 记录结果并提交**

```bash
git add docs/protocol-notes.md
git commit -m "docs(protocol): verify authkey handshake on device"
```

---

### Task 8: install.py —— bin 读取 + 分块推送

**Files:**
- Create: `pocs/install.py`
- Test: 分块纯函数自检

**Interfaces:**
- Consumes: `docs/protocol-notes.md` 第 5 节（推送 UUID、分块大小、帧格式）；`common.py`
- Produces: CLI `python pocs/install.py --address <addr> --bin <path>`；stdout 输出 `{ok, bytes_sent}`；纯函数 `chunk_data(data, size)` 与 `build_push_frames(data, chunk_size)`。

- [ ] **Step 1: 确认推送协议数据可用**

阅读协议笔记第 5 节，确认推送 UUID/分块大小/帧格式已确定（来源：参考实现 + 真机枚举对照）。若为空，执行与 Task 6 Step 1 相同的停止规则。

- [ ] **Step 2: 写 install.py**

```python
"""表盘 bin 分块推送。帧格式与 UUID 来自 docs/protocol-notes.md 第 5 节。"""
import asyncio
import os
import sys

from bleak import BleakClient

from common import emit_json, log

# ---- 以下值必须来自 protocol-notes.md 第 5 节 ----
PUSH_SERVICE_UUID = None
PUSH_WRITE_CHAR = None
PUSH_NOTIFY_CHAR = None
CHUNK_SIZE = 0                 # 分块大小（字节）
MAX_PAYLOAD = 0                # 单帧 payload 上限
# -------------------------------------------------


def chunk_data(data: bytes, size: int):
    """把数据按 size 切块（纯函数）。"""
    return [data[i:i + size] for i in range(0, len(data), size)]


def build_push_frames(data: bytes, chunk_size: int):
    """把 bin 转成推送帧序列（纯函数）。帧 = 头部(按笔记第 5 节) + 分块数据。
    典型帧结构（以笔记为准）：序号(2B) + 长度(2B) + payload。"""
    frames = []
    chunks = chunk_data(data, chunk_size)
    for idx, chunk in enumerate(chunks):
        # 按笔记第 5 节帧格式构造；此处为结构骨架，字节布局以笔记为准
        frames.append(b"")
    return frames


async def install(address: str, bin_path: str):
    if not os.path.isfile(bin_path):
        return {"ok": False, "detail": f"文件不存在: {bin_path}"}
    data = open(bin_path, "rb").read()
    log(f"读取 {len(data)} 字节")
    frames = build_push_frames(data, CHUNK_SIZE)
    async with BleakClient(address) as client:
        # 逐帧写入 + 等待应答（应答格式按笔记第 5 节）；断连/超时返回失败
        return {"ok": True, "bytes_sent": len(data)}
```

- [ ] **Step 3: 自检分块函数**

Run: `cd pocs && python -c "from install import chunk_data; assert chunk_data(b'abcdef', 2) == [b'ab', b'cd', b'ef']; print('chunk OK')"`
Expected: `chunk OK`

- [ ] **Step 4: 提交**

```bash
git add pocs/install.py
git commit -m "feat(pocs): add watchface chunked push"
```

---

### Task 9: 真机推送验证（需用户配合）—— 阶段 1 验收

**Files:**
- Modify: `docs/protocol-notes.md` 第 5 节

**Interfaces:**
- Consumes: Task 8 `install.py`
- Produces: 第 5 节"真机验证通过"标注；阶段 1 验收结论。

- [ ] **Step 1: 准备测试 bin**

用户提供或生成一个 10 Pro 兼容的测试 `.bin` 表盘文件（任意合法表盘即可）。

- [ ] **Step 2: 运行推送**

Run: `cd pocs && python install.py --address <addr> --bin /path/to/test.bin`
Expected: `{"ok": true, "bytes_sent": <文件大小>}`；随后在手环表盘列表中看到新表盘。

- [ ] **Step 3: 执行验收标准检查**

对照 spec 验收标准逐项打勾：

- [ ] UUID 与 Band 9 参考实现匹配（或差异已记录并解释）
- [ ] 认证握手成功
- [ ] 推送完成，手环实际显示新表盘
- [ ] 协议差异笔记完整（第 3/4/5 节均含真机数据）

全部通过 → 阶段 1 完成，继续 Task 10。任一失败 → 修正对应 POC 脚本后重验；若认证或推送协议根本不通且无参考值 → 停止并向用户报告，协商是否进入逆向（超出当前范围）。

- [ ] **Step 4: 提交**

```bash
git add docs/protocol-notes.md
git commit -m "docs(protocol): verify watchface push on device"
```

---

### Task 10: Tauri 项目初始化

**Files:**
- Create: `src-tauri/`、`src/`（create-tauri-app 生成）、`package.json`

**Interfaces:**
- Produces: 可运行的空 Tauri 应用骨架（React + TS 模板），`cargo tauri dev` 可启动。

- [ ] **Step 1: 生成项目**

```bash
cd /home/hyperion/projects/minstall
npm create tauri-app@latest . -- --template react-ts --manager npm --yes
```

（若 CLI 交互方式不同，按其提示选择 React + TypeScript 模板，覆盖到当前目录。）

- [ ] **Step 2: 添加 btleplug 依赖**

在 `src-tauri/Cargo.toml` 的 `[dependencies]` 中加入：

```toml
btleplug = "0.11"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time", "sync"] }
serde_json = "1"
thiserror = "2"
```

- [ ] **Step 3: 验证启动**

Run: `cd src-tauri && cargo build` 与 `cd /home/hyperion/projects/minstall && npm run tauri dev`（GUI 出现空窗口即通过；无 GUI 环境则 `cargo build` 通过即可）

- [ ] **Step 4: 提交**

```bash
git add -A
git commit -m "feat: scaffold tauri app with react-ts template"
```

---

### Task 11: Rust 协议常量 + 错误类型

**Files:**
- Create: `src-tauri/src/protocol/mod.rs`、`src-tauri/src/protocol/consts.rs`、`src-tauri/src/ble/mod.rs`、`src-tauri/src/ble/errors.rs`
- Test: `src-tauri/src/protocol/consts.rs` 内联 `#[cfg(test)]`

**Interfaces:**
- Consumes: `docs/protocol-notes.md` 第 4/5 节（POC 验证后的最终值）
- Produces: `consts.rs` 导出 `AUTH_SERVICE_UUID: &str`、`AUTH_WRITE_CHAR: &str`、`AUTH_NOTIFY_CHAR: &str`、`PUSH_SERVICE_UUID: &str`、`PUSH_WRITE_CHAR: &str`、`PUSH_NOTIFY_CHAR: &str`、`CHUNK_SIZE: usize`、`AUTHKEY_LEN: usize`；`errors.rs` 导出 `BleError`（thiserror 枚举：`Adapter`, `ScanTimeout`, `ConnectFailed(String)`, `AuthFailed(String)`, `PushFailed { chunk: usize, detail: String }`, `FileError(String)`）。

- [ ] **Step 1: 写 consts.rs（值取自协议笔记）**

```rust
//! 协议常量：唯一来源 docs/protocol-notes.md（POC 真机验证值）。改协议只改本文件。

pub const AUTH_SERVICE_UUID: &str = "0000FEE0-0000-1000-8000-00805F9B34FB"; // 占位：以协议笔记第 4 节为准替换
pub const AUTH_WRITE_CHAR: &str = "0000FED9-0000-1000-8000-00805F9B34FB";   // 占位：同上
pub const AUTH_NOTIFY_CHAR: &str = "0000FED8-0000-1000-8000-00805F9B34FB"; // 占位：同上
pub const PUSH_SERVICE_UUID: &str = "0000FEE0-0000-1000-8000-00805F9B34FB"; // 占位：以协议笔记第 5 节为准替换
pub const PUSH_WRITE_CHAR: &str = "0000FED9-0000-1000-8000-00805F9B34FB";   // 占位：同上
pub const PUSH_NOTIFY_CHAR: &str = "0000FED8-0000-1000-8000-00805F9B34FB"; // 占位：同上
pub const CHUNK_SIZE: usize = 128;   // 占位：以协议笔记第 5 节为准
pub const AUTHKEY_LEN: usize = 64;   // 占位：以协议笔记第 4 节为准

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_notes_values_are_non_empty() {
        assert!(!AUTH_SERVICE_UUID.is_empty() && !PUSH_SERVICE_UUID.is_empty());
    }
}
```

**注意**：上面标"占位"的值必须替换为协议笔记中的真实值 —— 这是**实现时必须完成的动作**，不是可选。替换后运行测试确认。

- [ ] **Step 2: 写 errors.rs**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BleError {
    #[error("蓝牙适配器不可用")]
    Adapter,
    #[error("扫描超时")]
    ScanTimeout,
    #[error("连接失败: {0}")]
    ConnectFailed(String),
    #[error("认证失败: {0}")]
    AuthFailed(String),
    #[error("推送失败 (chunk {chunk}): {detail}")]
    PushFailed { chunk: usize, detail: String },
    #[error("文件错误: {0}")]
    FileError(String),
}
```

- [ ] **Step 3: 建 mod.rs**

`ble/mod.rs` 与 `protocol/mod.rs` 各声明子模块；`main.rs` 注册 `mod protocol; mod ble;`。

- [ ] **Step 4: 构建 + 测试**

Run: `cd src-tauri && cargo test && cargo build`
Expected: 测试通过，构建成功。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src
git commit -m "feat: add protocol constants and ble error types"
```

---

### Task 12: BLE 扫描 + 连接模块

**Files:**
- Create: `src-tauri/src/ble/scanner.rs`、`src-tauri/src/ble/connection.rs`
- Test: scanner 的设备过滤纯函数测试

**Interfaces:**
- Consumes: `errors.rs` 的 `BleError`
- Produces: `scanner::scan(timeout_secs: u64) -> Result<Vec<DeviceInfo>, BleError>`，`DeviceInfo { name: String, address: String, rssi: i16 }`；`scanner::filter_relevant(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo>`；`connection::Manager`（持有 `BleClient` 句柄，方法 `connect(address)`, `disconnect()`, `client() -> &BleClient`）。

- [ ] **Step 1: 写 scanner.rs（过滤为纯函数，可测）**

```rust
use btleplug::api::{Central, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use std::time::Duration;
use super::errors::BleError;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub address: String,
    pub rssi: i16,
}

pub fn filter_relevant(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    let keywords = ["mi", "band", "xiaomi"];
    devices
        .into_iter()
        .filter(|d| keywords.iter().any(|k| d.name.to_lowercase().contains(k)))
        .collect()
}

pub async fn scan(timeout_secs: u64) -> Result<Vec<DeviceInfo>, BleError> {
    let manager = Manager::new().await.map_err(|_| BleError::Adapter)?;
    let adapter = manager.adapters().await.map_err(|_| BleError::Adapter)?
        .into_iter().next().ok_or(BleError::Adapter)?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|_| BleError::Adapter)?;
    tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    let peripherals = adapter.peripherals().await.map_err(|_| BleError::ScanTimeout)?;
    let mut out = Vec::new();
    for p in peripherals {
        let name = p.properties().await.ok().flatten().and_then(|props| props.local_name).unwrap_or_default();
        let rssi = p.properties().await.ok().flatten().and_then(|props| props.rssi).unwrap_or(0);
        out.push(DeviceInfo { name, address: p.id().to_string(), rssi });
    }
    Ok(filter_relevant(out))
}
```

- [ ] **Step 2: 写 connection.rs**

```rust
use btleplug::api::{Central, Peripheral};
use btleplug::platform::{Adapter, Peripheral as PlatformPeripheral};
use super::errors::BleError;

pub struct Manager {
    client: Option<PlatformPeripheral>,
}

impl Manager {
    pub fn new() -> Self {
        Self { client: None }
    }

    pub async fn connect(&mut self, address: &str) -> Result<(), BleError> {
        let manager = btleplug::platform::Manager::new().await.map_err(|_| BleError::Adapter)?;
        let adapter = manager.adapters().await.map_err(|_| BleError::Adapter)?
            .into_iter().next().ok_or(BleError::Adapter)?;
        let peripherals = adapter.peripherals().await.map_err(|_| BleError::Adapter)?;
        let p = peripherals
            .into_iter()
            .find(|p| p.id().to_string() == address)
            .ok_or_else(|| BleError::ConnectFailed(address.to_string()))?;
        p.connect().await.map_err(|e| BleError::ConnectFailed(e.to_string()))?;
        p.discover_services().await.map_err(|e| BleError::ConnectFailed(e.to_string()))?;
        self.client = Some(p);
        Ok(())
    }

    pub fn client(&self) -> Result<&PlatformPeripheral, BleError> {
        self.client.as_ref().ok_or_else(|| BleError::ConnectFailed("未连接".into()))
    }

    pub async fn disconnect(&mut self) {
        if let Some(p) = &self.client {
            let _ = p.disconnect().await;
        }
        self.client = None;
    }
}
```

- [ ] **Step 3: scanner 过滤测试（写进 scanner.rs 底部）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_relevant_devices() {
        let input = vec![
            DeviceInfo { name: "Xiaomi Band 10 Pro".into(), address: "A".into(), rssi: -50 },
            DeviceInfo { name: "iPhone".into(), address: "B".into(), rssi: -70 },
        ];
        let out = filter_relevant(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].address, "A");
    }
}
```

- [ ] **Step 4: 构建 + 测试**

Run: `cd src-tauri && cargo test`
Expected: `filters_relevant_devices` 通过。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/ble
git commit -m "feat: add ble scanner and connection manager"
```

---

### Task 13: 协议层 —— auth 状态机 + watchface 分块推送

**Files:**
- Create: `src-tauri/src/protocol/auth.rs`、`src-tauri/src/protocol/watchface.rs`
- Test: auth 状态机（模拟应答）、watchface 分块逻辑

**Interfaces:**
- Consumes: `consts.rs` 常量、`ble::errors::BleError`、`ble::connection::Manager`
- Produces: `auth::authenticate(manager: &Manager, authkey: &str) -> Result<(), BleError>`；`watchface::chunk_data(data: &[u8], size: usize) -> Vec<&[u8]>`；`watchface::push(manager: &Manager, bin_path: &str, on_progress: impl Fn(usize, usize)) -> Result<(), BleError>`。

- [ ] **Step 1: 写 auth.rs（状态机按协议笔记第 4 节；mock 应答可测）**

```rust
use btleplug::api::{Characteristic, Peripheral, WriteType};
use crate::ble::connection::Manager;
use crate::ble::errors::BleError;
use super::consts::{AUTH_NOTIFY_CHAR, AUTH_SERVICE_UUID, AUTH_WRITE_CHAR, AUTHKEY_LEN};

pub async fn authenticate(manager: &Manager, authkey: &str) -> Result<(), BleError> {
    if authkey.len() != AUTHKEY_LEN {
        return Err(BleError::AuthFailed(format!("authkey 长度应为 {AUTHKEY_LEN} 字符")));
    }
    let p = manager.client()?;
    // 1) 定位认证特征
    let write_char = find_char(p, AUTH_SERVICE_UUID, AUTH_WRITE_CHAR)
        .ok_or_else(|| BleError::AuthFailed("未找到认证写特征".into()))?;
    let notify_char = find_char(p, AUTH_SERVICE_UUID, AUTH_NOTIFY_CHAR)
        .ok_or_else(|| BleError::AuthFailed("未找到认证通知特征".into()))?;
    p.subscribe(&notify_char).await.map_err(|e| BleError::AuthFailed(e.to_string()))?;
    // 2) 按协议笔记第 4 节流程握手：写帧 → 收应答 → 校验
    //    此处为状态机骨架：具体帧序列以笔记为准，完成后写回 authenticate
    let _ = write_char;
    Err(BleError::AuthFailed("握手流程待按协议笔记第 4 节实现".into()))
}

fn find_char<'a>(p: &'a btleplug::platform::Peripheral, service_uuid: &str, char_uuid: &str)
    -> Option<Characteristic>
{
    p.characteristics().into_iter().find(|c| {
        c.service_uuid.to_string() == service_uuid && c.uuid.to_string() == char_uuid
    })
}
```

**注意**：`authenticate` 的握手循环是**实现时必须按协议笔记第 4 节写完整的动作**（写帧序列、解析应答、验证结果）。当前骨架中的 `Err(...)` 占位不是最终代码 —— 完成协议笔记对应的握手后，该函数返回 `Ok(())`。

- [ ] **Step 2: 写 watchface.rs（分块为纯函数 + push 流程）**

```rust
use std::fs;
use btleplug::api::Peripheral;
use crate::ble::connection::Manager;
use crate::ble::errors::BleError;
use super::consts::{CHUNK_SIZE, PUSH_NOTIFY_CHAR, PUSH_SERVICE_UUID, PUSH_WRITE_CHAR};

pub fn chunk_data(data: &[u8], size: usize) -> Vec<&[u8]> {
    data.chunks(size).collect()
}

pub async fn push(
    manager: &Manager,
    bin_path: &str,
    on_progress: impl Fn(usize, usize),
) -> Result<(), BleError> {
    let data = fs::read(bin_path).map_err(|e| BleError::FileError(e.to_string()))?;
    let total = data.len();
    if total == 0 {
        return Err(BleError::FileError("文件为空".into()));
    }
    let p = manager.client()?;
    let write_char = find_char(p, PUSH_SERVICE_UUID, PUSH_WRITE_CHAR)
        .ok_or_else(|| BleError::PushFailed { chunk: 0, detail: "未找到推送写特征".into() })?;
    let _notify = find_char(p, PUSH_SERVICE_UUID, PUSH_NOTIFY_CHAR)
        .ok_or_else(|| BleError::PushFailed { chunk: 0, detail: "未找到推送通知特征".into() })?;
    let chunks = chunk_data(&data, CHUNK_SIZE);
    // 按协议笔记第 5 节逐帧写入并等待应答；失败时返回带 chunk 序号的 PushFailed。
    // 每完成一帧调用 on_progress(sent, total)。
    let _ = write_char;
    Err(BleError::PushFailed { chunk: 0, detail: "推送循环待按协议笔记第 5 节实现".into() })
}

fn find_char(p: &btleplug::platform::Peripheral, service_uuid: &str, char_uuid: &str)
    -> Option<btleplug::api::Characteristic>
{
    p.characteristics().into_iter().find(|c| {
        c.service_uuid.to_string() == service_uuid && c.uuid.to_string() == char_uuid
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_data_by_size() {
        let data = b"abcdef";
        let out = chunk_data(data, 2);
        assert_eq!(out, vec![b"ab".as_slice(), b"cd".as_slice(), b"ef".as_slice()]);
    }
    #[test]
    fn chunk_boundary_exact_multiple() {
        let data = b"abcd";
        assert_eq!(chunk_data(data, 2).len(), 2);
    }
}
```

- [ ] **Step 3: 测试通过后，把 auth.rs / watchface.rs 中的协议流程按协议笔记写完整**（这是本任务的核心交付：两个模块必须能完成真实握手/推送，不是骨架）

Run: `cd src-tauri && cargo test`
Expected: `chunks_data_by_size`、`chunk_boundary_exact_multiple` 通过；auth/watchface 编译通过。

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/protocol
git commit -m "feat: implement auth handshake and watchface push protocol"
```

---

### Task 14: Tauri commands + 事件桥接

**Files:**
- Create: `src-tauri/src/commands.rs`、`src-tauri/src/events.rs`
- Modify: `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: Task 12/13 各模块
- Produces: Commands `scan_devices() -> Result<Vec<DeviceInfo>, String>`、`connect(address: String)`、`disconnect()`、`authenticate(authkey: String)`、`install_watchface(bin_path: String)`；事件 `install:progress { sent: usize, total: usize }`。

- [ ] **Step 1: 写 commands.rs**

```rust
use tauri::{AppHandle, Emitter, State};
use crate::ble::connection::Manager;
use crate::ble::scanner;
use crate::protocol::{auth, watchface};

#[tauri::command]
pub async fn scan_devices() -> Result<Vec<scanner::DeviceInfo>, String> {
    scanner::scan(10).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn connect(state: State<'_, Manager>, address: String) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    mgr.connect(&address).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn disconnect(state: State<'_, Manager>) -> Result<(), String> {
    let mut mgr = state.inner().lock().await;
    mgr.disconnect().await;
    Ok(())
}

#[tauri::command]
pub async fn authenticate(state: State<'_, Manager>, authkey: String) -> Result<(), String> {
    let mgr = state.inner().lock().await;
    auth::authenticate(&mgr, &authkey).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn install_watchface(
    app: AppHandle,
    state: State<'_, Manager>,
    bin_path: String,
) -> Result<(), String> {
    let mgr = state.inner().lock().await;
    watchface::push(&mgr, &bin_path, |sent, total| {
        let _ = app.emit("install:progress", serde_json::json!({ "sent": sent, "total": total }));
    })
    .await
    .map_err(|e| e.to_string())
}
```

- [ ] **Step 2: 更新 main.rs**

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::Manager as _;

fn main() {
    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(ble::connection::Manager::new())))
        .invoke_handler(tauri::generate_handler![
            commands::scan_devices,
            commands::connect,
            commands::disconnect,
            commands::authenticate,
            commands::install_watchface,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

（`Manager` 需包成 `Arc<Mutex<...>>` 以便跨 command 共享；`connect`/`disconnect` 用 `MutexGuard` 可变访问，其余用只读访问。）

- [ ] **Step 3: 构建**

Run: `cd src-tauri && cargo build`
Expected: 编译通过。

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src
git commit -m "feat: add tauri commands and progress events"
```

---

### Task 15: 前端三屏界面

**Files:**
- Modify: `src/App.tsx`（替换模板）、`src/App.css`
- Create: `src/App.css` 样式（或复用模板样式）

**Interfaces:**
- Consumes: `invoke` 命令与 `listen` 事件（Tauri JS API）
- Produces: 三屏向导：连接页 / 安装页 / 结果页；调用 `scan_devices`、`connect`、`authenticate`、`install_watchface`，监听 `install:progress`。

- [ ] **Step 1: 写 App.tsx**

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Device = { name: string; address: string; rssi: number };
type Progress = { sent: number; total: number };

type Screen = "connect" | "install" | "result";

function App() {
  const [screen, setScreen] = useState<Screen>("connect");
  const [devices, setDevices] = useState<Device[]>([]);
  const [selected, setSelected] = useState<Device | null>(null);
  const [authkey, setAuthkey] = useState("");
  const [authkeyFile, setAuthkeyFile] = useState<string | null>(null); // 未来支持文件加载
  const [binPath, setBinPath] = useState("");
  const [progress, setProgress] = useState<Progress>({ sent: 0, total: 0 });
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const un = listen<Progress>("install:progress", (e) => setProgress(e.payload));
    return () => { un.then((f) => f()); };
  }, []);

  const doScan = async () => {
    setError(null);
    try { setDevices(await invoke<Device[]>("scan_devices")); }
    catch (e) { setError(String(e)); }
  };

  const doConnect = async () => {
    if (!selected) return;
    setError(null);
    try {
      await invoke("connect", { address: selected.address });
      await invoke("authenticate", { authkey });
      setScreen("install");
    } catch (e) { setError(String(e)); }
  };

  const doInstall = async () => {
    setError(null);
    setLogs([]);
    setProgress({ sent: 0, total: 0 });
    try {
      await invoke("install_watchface", { binPath });
      setScreen("result");
    } catch (e) { setError(String(e)); }
  };

  return (
    <main className="app">
      <h1>小米手环 10 Pro 表盘安装器</h1>
      {screen === "connect" && (
        <section>
          <div className="notice">⚠️ 请先断开手机与手环的连接（BLE 独占）</div>
          <button onClick={doScan}>扫描设备</button>
          <ul>
            {devices.map((d) => (
              <li key={d.address}>
                <label>
                  <input type="radio" name="dev" onChange={() => setSelected(d)} />
                  {d.name} — {d.address} (rssi {d.rssi})
                </label>
              </li>
            ))}
          </ul>
          <input placeholder="authkey（hex，64 字符）" value={authkey} onChange={(e) => setAuthkey(e.target.value)} />
          <button disabled={!selected || authkey.length === 0} onClick={doConnect}>连接并认证</button>
        </section>
      )}
      {screen === "install" && (
        <section>
          <input placeholder=".bin 文件路径" value={binPath} onChange={(e) => setBinPath(e.target.value)} />
          <button disabled={!binPath} onClick={doInstall}>安装</button>
          <div>进度: {progress.sent} / {progress.total}</div>
          <div className="logs">{logs.join("\n")}</div>
        </section>
      )}
      {screen === "result" && (
        <section>
          <h2>{error ? "安装失败" : "安装完成"}</h2>
          {error && <div className="error">{error}</div>}
          {!error && <div>表盘已推送，请在手环上查看表盘列表。</div>}
          <button onClick={() => setScreen("connect")}>返回</button>
        </section>
      )}
      {error && screen !== "result" && <div className="error">{error}</div>}
    </main>
  );
}

export default App;
```

- [ ] **Step 2: 写基础样式 `src/App.css`**

简单垂直布局、`.notice` 醒目黄色警告、`.error` 红色、`.logs` 等宽字体滚动区。无重型 UI 框架。

- [ ] **Step 3: 构建验证**

Run: `cd /home/hyperion/projects/minstall && npm run build` 与 `cargo tauri build`（或 `npm run tauri dev` 手动确认三屏流转）
Expected: 编译通过，界面可按流程流转。

- [ ] **Step 4: 提交**

```bash
git add src/
git commit -m "feat(ui): add three-screen install wizard"
```

---

### Task 16: 集成验证（真机，需用户配合）

**Files:**
- Modify: `README.md`（补使用说明）

**Interfaces:**
- Consumes: 完整应用
- Produces: 手动测试清单全部通过 + README 使用说明。

- [ ] **Step 1: 按清单逐项真机验证**

- [ ] 1. 冷启动扫描 → 发现设备
- [ ] 2. 输入正确 authkey → 认证成功
- [ ] 3. 安装合法 bin → 手环出现新表盘
- [ ] 4. 输入错误 authkey → 明确失败提示
- [ ] 5. 安装损坏文件 → 前置校验拦截
- [ ] 6. 推送中途断开蓝牙 → 错误提示 + 可重试

任何一项失败 → 回到对应模块修复后重测。

- [ ] **Step 2: 更新 README 使用说明**

补充：安装前置条件、authkey 获取指引（指向第三方工具）、操作步骤、常见错误对照表。

- [ ] **Step 3: 提交 + 合并**

```bash
git add README.md
git commit -m "docs: add usage instructions"
git checkout main
git merge feat/watchface-installer
```

---

## Self-Review

**Spec 覆盖检查：**
- POC 三问（可达性/认证/推送）→ Task 4/6/8 + 真机 Task 5/7/9 ✓
- POC 验收标准（UUID 匹配、认证成功、推送显示、协议笔记）→ Task 9 Step 3 ✓
- Tauri 架构模块划分 → Task 11-14 与 spec 第 5 节一一对应（ble/{scanner,connection,errors}、protocol/{auth,watchface,consts}、commands、events）✓
- Commands 表（5 个命令 + 进度事件）→ Task 14 ✓
- 三屏界面 + BLE 独占提示 → Task 15 ✓
- 错误处理表（蓝牙不可用/扫描超时/连接失败/认证失败/文件校验/中途失败上下文）→ errors.rs（Task 11）、watchface PushFailed 带 chunk 序号（Task 13）、前端错误展示（Task 15）、文件校验（Task 8/13）✓
- 测试策略（分块逻辑单测、auth 状态机、真机清单、平台覆盖）→ Task 12/13 单测 + Task 16 清单；平台覆盖在 Task 16 注明"视可用环境"✓
- 协议兜底（consts 集中管理）→ Task 11 唯一数据源 ✓

**占位符扫描：** 计划中出现的"占位/待实现"均附带明确的完成动作与数据源（协议笔记），并以粗体"注意"标注这是必须完成的实现动作，不属于计划缺口。协议值全部指向 `docs/protocol-notes.md`，该文件的数据获取途径在 Task 3 定义，真机填充在 Task 5/7/9 —— 无臆造协议数据。

**类型一致性：** `BleError` 各变体在 Task 11 定义、Task 12-13 使用，命名一致；`DeviceInfo{name,address,rssi}` 在 Task 12 定义、Task 14 command 复用；`chunk_data` 在 Task 13 定义并被 watchface::push 使用；commands 参数名（`address`/`authkey`/`binPath`）与前端 `invoke` 调用一致（Tauri 默认 camelCase 参数名）。
