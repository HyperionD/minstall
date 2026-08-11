"""authkey 认证握手（V2 协议，PROTOBUF 通道，加密前明文阶段）。

协议值来源：
  - UUID / V2 帧格式 / 认证序列：docs/protocol-notes.md 第 3、4 节
    （UUID 真机确认；帧格式、SessionConfig TLV 与认证序列来自参考实现，待真机确认）
  - 算法细节：.superpowers/sdd/2026-08-11-watchface-installer/task-3-reference-notes.md
  - 参考实现：Kodo / Gadgetbridge（AGPL-3.0，仅作协议参考，本文件代码自行编写）

用法：
  python pocs/auth.py --self-test
  python pocs/auth.py --address <BLE地址> --authkey <32hex> [--phone-name X] [--api-level 34] [--region CN]

stdout 输出一行 JSON：{"authenticated": bool, "detail": str}。
"""
import asyncio
import hashlib
import hmac
import os
import platform
import struct
import sys
import time

try:
    from Crypto.Cipher import AES  # pycryptodome
except ImportError:
    raise SystemExit("缺少 pycryptodome：pip install -r pocs/requirements.txt")

from common import emit_json, log

# ---------------------------------------------------------------------------
# 协议常量（来源标注见 docs/protocol-notes.md 第 3、4 节）
# ---------------------------------------------------------------------------

# 认证 service / 特征 UUID —— 真机确认（第 3 节 GATT 枚举）
AUTH_SERVICE_UUID = "0000fe95-0000-1000-8000-00805f9b34fb"
AUTH_WRITE_CHAR = "0000005f-0000-1000-8000-00805f9b34fb"    # V2 TX（write-without-response）
AUTH_NOTIFY_CHAR = "0000005e-0000-1000-8000-00805f9b34fb"   # V2 RX（notify）
CCC_UUID = "00002902-0000-1000-8000-00805f9b34fb"

# authkey：32 hex 字符 = 16 字节；支持 "0x" 前缀（第 4 节，待真机确认）
AUTHKEY_LEN = 32

# V2 帧（第 4 节，待真机确认）
PREAMBLE = b"\xa5\xa5"
PT_ACK = 1
PT_SESSION_CONFIG = 2
PT_DATA = 3

# DATA 包 payload 的 channel / opCode（第 4 节，待真机确认）
CH_PROTOBUF = 1   # 加密
CH_DATA = 2       # 明文
CH_ACTIVITY = 5   # 加密
OP_PLAINTEXT = 1
OP_ENCRYPTED = 2

# SessionConfig opcode（第 4 节，待真机确认；TLV 参数值来自参考实现）
OP_START_SESSION_REQUEST = 1
OP_START_SESSION_RESPONSE = 2
_TLV_VERSION = (1, b"\x01\x00\x00")          # KEY_VERSION 01.00.00
_TLV_MAX_PACKET_SIZE = (2, b"\x00\xfc")      # 0xFC00
_TLV_TX_WIN = (3, b"\x20\x00")               # 32
_TLV_SEND_TIMEOUT = (4, b"\x10\x27")         # 10000 ms

# Command type / subtype（第 4 节，待真机确认）
CMD_TYPE_AUTH = 1
CMD_SUBTYPE_NONCE = 26   # CMD_NONCE
CMD_SUBTYPE_AUTH = 27    # CMD_AUTH
CMD_SUBTYPE_CONNECTED = 5

# 握手流程：每步 (step_name, ...)，见 build_auth_frames
AUTH_FLOW = [
    ("session_start",),
    ("expect", "session_response"),
    ("phone_nonce",),
    ("expect", "watch_nonce"),
    ("auth_step3",),
    ("expect", "auth_done"),
]

# ---------------------------------------------------------------------------
# CRC-16/ARC（第 4 节：poly 0x8005, init 0, 无 xor, refin, refout）
# ---------------------------------------------------------------------------


def crc16_arc(data: bytes) -> int:
    """CRC-16/ARC。标准校验值：crc16_arc(b"123456789") == 0xBB3D。

    与参考实现 XiaomiSppPacketV2.crc16Arc（左移 + 最终位反转式）等价，自检中交叉验证。
    """
    crc = 0
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    return crc & 0xFFFF


# ---------------------------------------------------------------------------
# V2 帧编解码（第 4 节）
# ---------------------------------------------------------------------------


def encode_v2_frame(packet_type: int, seq: int, payload: bytes = b"") -> bytes:
    """V2 帧编码：A5A5 + type(低 nibble) + seq u8 + len u16LE + crc16 u16LE + payload。"""
    header = (
        PREAMBLE
        + bytes([packet_type & 0x0F, seq & 0xFF])
        + struct.pack("<H", len(payload))
        + struct.pack("<H", crc16_arc(payload))
    )
    return header + payload


def parse_v2_frame(data: bytes):
    """解析 data 开头的单帧。返回 (packet_type, seq, payload)。

    - 数据不足：返回 None（等待更多字节）
    - 帧头非法（preamble/CRC 不符）：抛 ValueError
    """
    if len(data) < 8:
        return None
    if data[0:2] != PREAMBLE:
        raise ValueError(f"preamble 不匹配: {data[:2].hex()}")
    packet_type = data[2] & 0x0F
    seq = data[3]
    payload_len = struct.unpack("<H", data[4:6])[0]
    given_crc = struct.unpack("<H", data[6:8])[0]
    if len(data) < 8 + payload_len:
        return None
    payload = bytes(data[8:8 + payload_len])
    if crc16_arc(payload) != given_crc:
        raise ValueError(f"crc16 校验失败: 期望 {given_crc:#06x}")
    return (packet_type, seq, payload)


class V2Accumulator:
    """BLE 通知字节流 → 完整 V2 帧列表（思路同参考 V2PacketAccumulator，自行实现）。

    非法帧头时丢弃前缀字节、对齐到下一个 preamble。
    """

    def __init__(self):
        self.buf = bytearray()

    def feed(self, data: bytes):
        self.buf += data
        frames = []
        while self.buf:
            try:
                frame = parse_v2_frame(bytes(self.buf))
            except ValueError:
                self._resync()
                continue  # 丢弃非法前缀后继续处理剩余字节
            if frame is None:
                break  # 帧不完整，等更多数据
            frames.append(frame)
            del self.buf[:8 + len(frame[2])]
        return frames

    def _resync(self):
        idx = self.buf.find(PREAMBLE, 1)
        if idx < 0:
            self.buf = bytearray()
        else:
            del self.buf[:idx]


def build_ack_frame(seq: int) -> bytes:
    return encode_v2_frame(PT_ACK, seq)


def build_session_config(opcode: int, seq: int = 0) -> bytes:
    """SessionConfig 帧。payload = [opcode u8][TLV...]（TLV 参数来自参考实现，待真机确认）。"""
    tlvs = b""
    for key, value in (_TLV_VERSION, _TLV_MAX_PACKET_SIZE, _TLV_TX_WIN, _TLV_SEND_TIMEOUT):
        tlvs += bytes([key]) + struct.pack("<H", len(value)) + value
    return encode_v2_frame(PT_SESSION_CONFIG, seq, bytes([opcode]) + tlvs)


def build_protobuf_frame(seq: int, cmd_bytes: bytes, encrypt: bool = False, key: bytes | None = None) -> bytes:
    """PROTOBUF 通道 Data 帧。加密前（握手阶段）encrypt=False；后续加密命令 encrypt=True。"""
    opcode = OP_ENCRYPTED if encrypt else OP_PLAINTEXT
    body = encrypt_v2(key, cmd_bytes) if encrypt else cmd_bytes
    payload = bytes([CH_PROTOBUF & 0x0F, opcode & 0xFF]) + body
    return encode_v2_frame(PT_DATA, seq, payload)


# ---------------------------------------------------------------------------
# protobuf 编码（手写 varint；消息结构见参考 xiaomi.proto）
# ---------------------------------------------------------------------------


def _varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _field_varint(num: int, value: int) -> bytes:
    return _varint((num << 3) | 0) + _varint(value)


def _field_bytes(num: int, data: bytes) -> bytes:
    return _varint((num << 3) | 2) + _varint(len(data)) + data


def _field_message(num: int, data: bytes) -> bytes:
    return _field_bytes(num, data)


def _field_fixed32(num: int, value: float) -> bytes:
    return _varint((num << 3) | 5) + struct.pack("<f", value)


def _read_varint(buf: bytes, pos: int):
    result = 0
    shift = 0
    while True:
        if pos >= len(buf):
            raise ValueError("varint 截断")
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not b & 0x80:
            return (result, pos)
        shift += 7
        if shift > 63:
            raise ValueError("varint 过长")


def _parse_fields(buf: bytes, pos: int = 0):
    """扫描 protobuf 字段，返回 [(field_num, wire_type, value)]。

    value：varint=int，length-delimited=bytes，fixed32/fixed64=bytes。
    """
    fields = []
    while pos < len(buf):
        tag, pos = _read_varint(buf, pos)
        num, wt = tag >> 3, tag & 7
        if wt == 0:
            v, pos = _read_varint(buf, pos)
            fields.append((num, wt, v))
        elif wt == 1:
            fields.append((num, wt, bytes(buf[pos:pos + 8])))
            pos += 8
        elif wt == 2:
            ln, pos = _read_varint(buf, pos)
            fields.append((num, wt, bytes(buf[pos:pos + ln])))
            pos += ln
        elif wt == 5:
            fields.append((num, wt, bytes(buf[pos:pos + 4])))
            pos += 4
        else:
            raise ValueError(f"不支持的 wire type {wt}")
    return fields


def encode_phone_nonce(nonce: bytes) -> bytes:
    """PhoneNonce{nonce=1}。"""
    return _field_bytes(1, nonce)


def encode_watch_nonce(nonce: bytes, hmac_: bytes) -> bytes:
    """WatchNonce{nonce=1, hmac=2}。"""
    return _field_bytes(1, nonce) + _field_bytes(2, hmac_)


def encode_auth_step3(encrypted_nonces: bytes, encrypted_device_info: bytes) -> bytes:
    """AuthStep3{encryptedNonces=1, encryptedDeviceInfo=2}。"""
    return _field_bytes(1, encrypted_nonces) + _field_bytes(2, encrypted_device_info)


def encode_auth_device_info(unknown1: int, phone_api_level: float, phone_name: bytes,
                            unknown3: int, region: bytes) -> bytes:
    """AuthDeviceInfo{unknown1=1(uint32, 需显式序列化 0), phoneApiLevel=2(float),
    phoneName=3(string), unknown3=4(uint32=224), region=5(2 字母大写)}。"""
    return (
        _field_varint(1, unknown1)
        + _field_fixed32(2, phone_api_level)
        + _field_bytes(3, phone_name)
        + _field_varint(4, unknown3)
        + _field_bytes(5, region)
    )


def encode_command(type_: int, subtype: int | None = None, auth: bytes | None = None) -> bytes:
    """Command{type=1, subtype=2, auth=3}。"""
    out = _field_varint(1, type_)
    if subtype is not None:
        out += _field_varint(2, subtype)
    if auth is not None:
        out += _field_message(3, auth)
    return out


def encode_command_phone_nonce(nonce: bytes) -> bytes:
    """Command{type=1, subtype=26, Auth{phoneNonce=30}}。"""
    auth = _field_message(30, encode_phone_nonce(nonce))
    return encode_command(CMD_TYPE_AUTH, CMD_SUBTYPE_NONCE, auth)


def encode_command_watch_nonce(nonce: bytes, hmac_: bytes) -> bytes:
    """Command{type=1, subtype=26, Auth{watchNonce=31}}（收包构造，自检用）。"""
    auth = _field_message(31, encode_watch_nonce(nonce, hmac_))
    return encode_command(CMD_TYPE_AUTH, CMD_SUBTYPE_NONCE, auth)


def encode_command_auth_step3(encrypted_nonces: bytes, encrypted_device_info: bytes) -> bytes:
    """Command{type=1, subtype=27, Auth{authStep3=32}}。"""
    auth = _field_message(32, encode_auth_step3(encrypted_nonces, encrypted_device_info))
    return encode_command(CMD_TYPE_AUTH, CMD_SUBTYPE_AUTH, auth)


def parse_command(data: bytes):
    """解析收包 Command。返回 dict，键按存在性出现：
    type / subtype / phone_nonce / watch_nonce / watch_hmac。解析失败返回 None。"""
    try:
        fields = _parse_fields(data)
    except ValueError:
        return None
    cmd = {}
    auth = None
    for num, wt, val in fields:
        if num == 1 and wt == 0:
            cmd["type"] = val
        elif num == 2 and wt == 0:
            cmd["subtype"] = val
        elif num == 3 and wt == 2:
            auth = val
    if auth is None:
        return cmd
    try:
        auth_fields = {n: v for n, _w, v in _parse_fields(auth)}
    except ValueError:
        return cmd
    if isinstance(auth_fields.get(31), bytes):
        try:
            wn = {n: v for n, _w, v in _parse_fields(auth_fields[31])}
        except ValueError:
            wn = {}
        if isinstance(wn.get(1), bytes) and isinstance(wn.get(2), bytes):
            cmd["watch_nonce"] = wn[1]
            cmd["watch_hmac"] = wn[2]
    if isinstance(auth_fields.get(30), bytes):
        try:
            pn = {n: v for n, _w, v in _parse_fields(auth_fields[30])}
        except ValueError:
            pn = {}
        if isinstance(pn.get(1), bytes):
            cmd["phone_nonce"] = pn[1]
    return cmd


# ---------------------------------------------------------------------------
# 认证算法（task-3-reference-notes.md / XiaomiCrypto.kt，自行编写）
# ---------------------------------------------------------------------------


def hmac_sha256(key: bytes, msg: bytes) -> bytes:
    return hmac.new(key, msg, hashlib.sha256).digest()


def derive_session(secret16: bytes, phone_nonce16: bytes, watch_nonce16: bytes) -> bytes:
    """派生 64B 会话材料：(0-15)decKey (16-31)encKey (32-35)decNonce (36-39)encNonce。

    HMAC-SHA256(key=phoneNonce||watchNonce, msg=secret) → intermediate；
    再以 intermediate 为 key 做 tmp||"miwear-auth"||counter 计数器扩展（counter 从 1 起）。
    """
    if not (len(secret16) == 16 and len(phone_nonce16) == 16 and len(watch_nonce16) == 16):
        raise ValueError("secret/phoneNonce/watchNonce 均须 16 字节")
    intermediate = hmac_sha256(phone_nonce16 + watch_nonce16, secret16)
    out = bytearray()
    tmp = b""
    counter = 1
    while len(out) < 64:
        tmp = hmac_sha256(intermediate, tmp + b"miwear-auth" + bytes([counter]))
        out += tmp
        counter += 1
    return bytes(out[:64])


def verify_watch_hmac(dec_key: bytes, watch_nonce: bytes, phone_nonce: bytes, watch_hmac: bytes) -> bool:
    """HMAC-SHA256(key=decKey, msg=watchNonce||phoneNonce) == watchHmac。"""
    expected = hmac_sha256(dec_key, watch_nonce + phone_nonce)
    return hmac.compare_digest(expected, watch_hmac)


def phone_ack(enc_key: bytes, phone_nonce: bytes, watch_nonce: bytes) -> bytes:
    """HMAC-SHA256(key=encKey, msg=phoneNonce||watchNonce)，供 AuthStep3.encryptedNonces。"""
    return hmac_sha256(enc_key, phone_nonce + watch_nonce)


def encrypt_v1(key: bytes, enc_nonce4: bytes, counter: int, payload: bytes) -> bytes:
    """AES-128-CCM，nonce=(encNonce4, 4×0x00, counter u32LE)，macBits=32。输出 ct||tag(4B)。"""
    nonce = enc_nonce4 + b"\x00" * 4 + struct.pack("<I", counter)
    cipher = AES.new(key, AES.MODE_CCM, nonce=nonce, mac_len=4)
    ct, tag = cipher.encrypt_and_digest(payload)
    return ct + tag


def decrypt_v1(key: bytes, enc_nonce4: bytes, ciphertext: bytes) -> bytes:
    """CCM 解密（同一会话 nonce 前缀、counter=0、macBits=32），供自检往返。"""
    nonce = enc_nonce4 + b"\x00" * 4 + struct.pack("<I", 0)
    cipher = AES.new(key, AES.MODE_CCM, nonce=nonce, mac_len=4)
    return cipher.decrypt_and_verify(ciphertext[:-4], ciphertext[-4:])


def encrypt_v2(key: bytes, payload: bytes) -> bytes:
    """AES-128-CTR，key 即 IV（legacy quirk，第 4 节）。"""
    return AES.new(key, AES.MODE_CTR, nonce=b"", initial_value=key).encrypt(payload)


def decrypt_v2(key: bytes, ciphertext: bytes) -> bytes:
    """AES-128-CTR 解密（key 即 IV）。"""
    return AES.new(key, AES.MODE_CTR, nonce=b"", initial_value=key).decrypt(ciphertext)


def parse_authkey(authkey_hex: str) -> bytes | None:
    """authkey：32 hex 字符 = 16 字节，支持 "0x" 前缀（第 4 节）。非法返回 None。"""
    if not authkey_hex:
        return None
    s = authkey_hex.strip()
    if s.lower().startswith("0x"):
        s = s[2:]
    if len(s) != AUTHKEY_LEN:
        return None
    try:
        return bytes.fromhex(s)
    except ValueError:
        return None


def default_device_info() -> dict:
    """AuthDeviceInfo 默认值（参考实现字段；apiLevel/name/region 桌面端可取本机值）。"""
    lang = os.environ.get("LC_ALL") or os.environ.get("LANG") or ""
    parts = lang.split(".")[0].split("_")
    region = parts[1][:2].upper() if len(parts) >= 2 and parts[1] else "CN"
    return {
        "unknown1": 0,
        "phoneApiLevel": 34.0,
        "phoneName": platform.node() or "PC",
        "unknown3": 224,
        "region": region,
    }


# ---------------------------------------------------------------------------
# 流程 API（task-6-brief 契约）
# ---------------------------------------------------------------------------


def build_auth_step3_frame(secret: bytes, phone_nonce: bytes, watch_nonce: bytes,
                           device_info: dict | None = None, seq: int = 1) -> bytes:
    """构造 AuthStep3 明文帧：deriveSession → phoneAck + encryptV1(AuthDeviceInfo)。"""
    if len(secret) != 16:
        raise ValueError("secret 须为 16 字节")
    if device_info is None:
        device_info = default_device_info()
    derived = derive_session(secret, phone_nonce, watch_nonce)
    enc_key = derived[16:32]
    enc_nonce4 = derived[36:40]
    info_bytes = encode_auth_device_info(
        device_info["unknown1"],
        float(device_info["phoneApiLevel"]),
        str(device_info["phoneName"]).encode("utf-8"),
        device_info["unknown3"],
        str(device_info["region"]).encode("utf-8"),
    )
    step3 = encode_command_auth_step3(
        phone_ack(enc_key, phone_nonce, watch_nonce),
        encrypt_v1(enc_key, enc_nonce4, 0, info_bytes),
    )
    return build_protobuf_frame(seq, step3, encrypt=False)


def build_auth_frames(authkey_hex: str, flow: list, phone_nonce: bytes | None = None,
                      watch_nonce: bytes | None = None, device_info: dict | None = None) -> list:
    """按 flow 生成握手帧序列（纯函数）。

    flow 每项：
      ("send", payload)      —— 原样透传
      ("session_start",)     —— SessionConfig START_SESSION_REQUEST（seq 固定 0）
      ("phone_nonce",)       —— Command{1,26,PhoneNonce} 明文 Data 帧
      ("auth_step3",)        —— Command{1,27,AuthStep3} 明文 Data 帧（需提供 watch_nonce）
      ("expect", tag)        —— 等待应答，帧位为 None

    phone_nonce / watch_nonce 缺省时随机生成。返回帧列表（expect 位为 None）。
    仅 "auth_step3" 步骤需要合法 authkey；纯透传（send）步骤不校验。
    """
    if phone_nonce is None:
        phone_nonce = os.urandom(16)
    if device_info is None:
        device_info = default_device_info()
    frames = []
    data_seq = 0
    secret = None
    for step in flow:
        name = step[0]
        if name == "send":
            frames.append(bytes(step[1]))
        elif name == "session_start":
            frames.append(build_session_config(OP_START_SESSION_REQUEST, seq=0))
        elif name == "phone_nonce":
            frames.append(build_protobuf_frame(data_seq, encode_command_phone_nonce(phone_nonce)))
            data_seq += 1
        elif name == "auth_step3":
            if secret is None:
                secret = parse_authkey(authkey_hex)
                if secret is None:
                    raise ValueError(f"authkey 应为 {AUTHKEY_LEN} hex 字符（可带 0x 前缀）")
            if watch_nonce is None:
                raise ValueError("auth_step3 步骤需要提供 watch_nonce")
            frames.append(build_auth_step3_frame(secret, phone_nonce, watch_nonce, device_info, seq=data_seq))
            data_seq += 1
        elif name == "expect":
            frames.append(None)
        else:
            raise ValueError(f"未知 flow 步骤: {name}")
    return frames


def parse_auth_response(data: bytes, flow: str, authkey_hex: str | None = None,
                        phone_nonce: bytes | None = None):
    """解析握手应答（纯函数）。返回 (ok: bool, detail: str)。

    flow 为当前 expect 步骤标签：session_response / watch_nonce / auth_done。
    watch_nonce 步骤提供 authkey_hex + phone_nonce 时会做完整 HMAC 验证。
    """
    try:
        frame = parse_v2_frame(data)
    except ValueError as e:
        return (False, f"帧无效: {e}")
    if frame is None:
        return (False, "帧不完整")
    packet_type, seq, payload = frame

    if flow == "session_response":
        if packet_type == PT_SESSION_CONFIG and payload and payload[0] == OP_START_SESSION_RESPONSE:
            return (True, f"session started (opcode={payload[0]}, seq={seq})")
        op = payload[0] if payload else -1
        return (False, f"未预期的 SessionConfig opcode={op}")

    if flow == "watch_nonce":
        cmd = _frame_command(frame)
        if not cmd or cmd.get("type") != CMD_TYPE_AUTH or cmd.get("subtype") != CMD_SUBTYPE_NONCE:
            got = f"{cmd.get('type')}/{cmd.get('subtype')}" if cmd else f"type={packet_type}"
            return (False, f"未预期的 Command {got}")
        wn, wh = cmd["watch_nonce"], cmd["watch_hmac"]
        detail = f"watch nonce={wn.hex()} hmac={wh.hex()}"
        if authkey_hex and phone_nonce is not None:
            secret = parse_authkey(authkey_hex)
            if secret is None:
                return (False, "authkey 非法")
            derived = derive_session(secret, phone_nonce, wn)
            if verify_watch_hmac(derived[0:16], wn, phone_nonce, wh):
                return (True, f"{detail}（HMAC 验证通过）")
            return (False, "watch HMAC 验证失败")
        return (True, f"{detail}（未提供 authkey，未验证 HMAC）")

    if flow == "auth_done":
        cmd = _frame_command(frame)
        if cmd and cmd.get("type") == CMD_TYPE_AUTH and cmd.get("subtype") in (CMD_SUBTYPE_AUTH, CMD_SUBTYPE_CONNECTED):
            return (True, f"认证完成 (subtype={cmd['subtype']})")
        got = f"{cmd.get('type')}/{cmd.get('subtype')}" if cmd else f"type={packet_type}"
        return (False, f"未预期的 Command {got}")

    return (False, f"未知 flow 标签 {flow}")


def _frame_command(frame):
    """从 Data 帧提取 Command dict；非 Data 帧或解析失败返回 None。"""
    packet_type, _seq, payload = frame
    if packet_type != PT_DATA or len(payload) < 2:
        return None
    return parse_command(payload[2:])


def _is_watch_nonce_cmd(frame):
    """命中 WatchNonce 应答（type=1, subtype=26, 含 watchNonce）时返回解析 dict，否则 None。"""
    cmd = _frame_command(frame)
    if cmd and cmd.get("type") == CMD_TYPE_AUTH and cmd.get("subtype") == CMD_SUBTYPE_NONCE \
            and "watch_nonce" in cmd:
        return cmd
    return None


def _is_auth_done_cmd(frame):
    """命中认证完成应答（type=1, subtype=27 或 5）时返回解析 dict，否则 None。"""
    cmd = _frame_command(frame)
    if cmd and cmd.get("type") == CMD_TYPE_AUTH and cmd.get("subtype") in \
            (CMD_SUBTYPE_AUTH, CMD_SUBTYPE_CONNECTED):
        return cmd
    return None


# ---------------------------------------------------------------------------
# BLE 握手（bleak 薄封装）
# ---------------------------------------------------------------------------


async def _pair(client) -> None:
    """尽力配对：bleak 3.x pair() 能力不足时提示手环确认。"""
    try:
        await asyncio.wait_for(client.pair(), timeout=10)
        log("已发起配对——如手环弹出确认请点击确认")
    except Exception as e:
        log(f"pair() 未成功（{type(e).__name__}: {e}）")
        log("提示：若手环未配对，请先在 bluetoothctl 中执行 'agent NoInputNoOutput' 后重试，"
            "并确认手环已在手机 App 解绑/关闭手机蓝牙")


async def authenticate(address: str, authkey_hex: str, device_info: dict | None = None,
                       timeout: float = 15.0) -> dict:
    """执行完整认证握手，返回 {"authenticated": bool, "detail": str}。"""
    secret = parse_authkey(authkey_hex)
    if secret is None:
        return {"authenticated": False, "detail": f"authkey 应为 {AUTHKEY_LEN} hex 字符（可带 0x 前缀）"}
    if device_info is None:
        device_info = default_device_info()

    from bleak import BleakClient  # 延迟导入，自检不依赖 bleak

    queue = asyncio.Queue()
    accumulator = V2Accumulator()

    def on_notify(_char, data):
        for frame in accumulator.feed(bytes(data)):
            queue.put_nowait(frame)

    async with BleakClient(address, timeout=timeout) as client:
        await _pair(client)

        # 校验 V2 service / 特征（需先配对成功，见 protocol-notes 第 3 节操作经验）
        svc = client.services.get_service(AUTH_SERVICE_UUID)
        if svc is None:
            return {"authenticated": False, "detail": f"未发现 V2 service {AUTH_SERVICE_UUID}（可能未配对）"}
        for uuid, name in ((AUTH_NOTIFY_CHAR, "V2 RX"), (AUTH_WRITE_CHAR, "V2 TX")):
            if svc.get_characteristic(uuid) is None:
                return {"authenticated": False, "detail": f"缺少特征 {name} {uuid}"}

        await client.start_notify(AUTH_NOTIFY_CHAR, on_notify)
        log(f"已订阅 {AUTH_NOTIFY_CHAR} 通知")

        async def send(frame: bytes) -> None:
            try:
                await client.write_gatt_char(AUTH_WRITE_CHAR, frame, response=False)
            except Exception as e:
                log(f"写入失败（{type(e).__name__}: {e}）")
                raise

        async def recv_until(predicate, label: str):
            """收帧直到 predicate 命中；Data 帧回 ACK，非目标帧跳过。超时返回 None。"""
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                remain = deadline - time.monotonic()
                try:
                    frame = await asyncio.wait_for(queue.get(), timeout=remain)
                except asyncio.TimeoutError:
                    log(f"等待 {label} 超时")
                    return None
                if frame[0] == PT_DATA:
                    try:
                        await send(build_ack_frame(frame[1]))
                    except Exception:
                        return None
                result = predicate(frame)
                if result is not None:
                    return result
                log(f"跳过非目标帧 (type={frame[0]} seq={frame[1]} payload={frame[2][:8].hex()}…)")
            return None

        # 1) START_SESSION_REQUEST（seq 固定 0，第 4 节）
        await send(build_session_config(OP_START_SESSION_REQUEST, seq=0))
        log("→ START_SESSION_REQUEST (seq=0)")
        got = await recv_until(
            lambda f: f[0] == PT_SESSION_CONFIG and f[2] and f[2][0] == OP_START_SESSION_RESPONSE,
            "START_SESSION_RESPONSE",
        )
        if got is None:
            return {"authenticated": False, "detail": "等待 START_SESSION_RESPONSE 超时"}

        # 2) PhoneNonce（明文 Data 帧，seq=0）
        phone_nonce = os.urandom(16)
        await send(build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
        log(f"→ PhoneNonce {phone_nonce.hex()}")

        # 3) 等 WatchNonce 并验证（第 4 节步骤 3）
        got = await recv_until(_is_watch_nonce_cmd, "WatchNonce")
        if got is None:
            return {"authenticated": False, "detail": "等待 WatchNonce 超时"}
        watch_nonce, watch_hmac = got["watch_nonce"], got["watch_hmac"]
        derived = derive_session(secret, phone_nonce, watch_nonce)
        dec_key, enc_key = derived[0:16], derived[16:32]
        enc_nonce4 = derived[36:40]
        if not verify_watch_hmac(dec_key, watch_nonce, phone_nonce, watch_hmac):
            return {"authenticated": False, "detail": "watch HMAC 验证失败"}
        log("watch HMAC 验证通过")

        # 4) AuthStep3（明文 Data 帧，seq=1；encryptV1 counter 从 0 起）
        info_bytes = encode_auth_device_info(
            device_info["unknown1"],
            float(device_info["phoneApiLevel"]),
            str(device_info["phoneName"]).encode("utf-8"),
            device_info["unknown3"],
            str(device_info["region"]).encode("utf-8"),
        )
        step3 = encode_command_auth_step3(
            phone_ack(enc_key, phone_nonce, watch_nonce),
            encrypt_v1(enc_key, enc_nonce4, 0, info_bytes),
        )
        await send(build_protobuf_frame(1, step3))
        log("→ AuthStep3")

        # 5) 等认证完成（type=1, subtype=27 或 5，第 4 节步骤 5）
        got = await recv_until(_is_auth_done_cmd, "认证完成应答")
        if got is None:
            return {"authenticated": False, "detail": "等待认证完成应答超时"}
        return {"authenticated": True, "detail": f"connected (subtype={got['subtype']})"}


# ---------------------------------------------------------------------------
# 自检与 CLI
# ---------------------------------------------------------------------------

# golden 字节：开发期已用 grpc_tools 生成的 xiaomi_pb2 与 cryptography 交叉验证
_G_PHONE_NONCE_CMD = "0801101a1a15f201120a1000000000000000000000000000000000"
_G_WATCH_NONCE_CMD = "0801101a1a37fa01340a100000000000000000000000000000000012201111111111111111111111111111111111111111111111111111111111111111"
_G_AUTH_STEP3_CMD = "0801101b1a378202340a100000000000000000000000000000000012201111111111111111111111111111111111111111111111111111111111111111"
_G_DEVICE_INFO = "080015000008421a03504f4320e0012a02434e"
_G_SESSION_PAYLOAD = "0101030001000002020000fc03020020000402001027"
_G_CCM_OUT = "eed3267521be8d33e40d8b9a42022d3cb1f7d769"
_G_CTR_OUT = "14f42f3141958835925097f1ac005c504d271f9022197c014f5a8dee8f83adaf"


def self_test() -> None:
    """纯函数自检：已知向量 + 编解码/加解密往返。"""
    # ---- CRC-16/ARC ----
    assert crc16_arc(b"123456789") == 0xBB3D, "CRC-16/ARC 标准校验值"
    # 与参考实现（左移 + 最终位反转式）等价性交叉验证
    def crc_kotlin_form(data):
        crc = 0
        for byte in data:
            for j in range(8):
                crc = (crc << 1) & 0xFFFFFFFF
                bit = ((crc >> 16) & 1) ^ ((byte >> j) & 1)
                if bit == 1:
                    crc = (crc ^ 0x8005) & 0xFFFFFFFF
        rev = int(f"{crc:032b}"[::-1], 2)
        return (rev >> 16) & 0xFFFF
    for data in (b"", b"\x00", b"\xa5\xa5", bytes(range(256)), b"hello world"):
        assert crc16_arc(data) == crc_kotlin_form(data), f"CRC 等价性: {data[:8].hex()}"

    # ---- V2 帧编解码往返 ----
    cases = [
        (PT_ACK, 5, b""),
        (PT_SESSION_CONFIG, 0, bytes.fromhex(_G_SESSION_PAYLOAD)),
        (PT_DATA, 3, bytes([0x01, 0x01]) + bytes(range(30))),
    ]
    for packet_type, seq, payload in cases:
        assert parse_v2_frame(encode_v2_frame(packet_type, seq, payload)) == (packet_type, seq, payload)
    # golden 帧
    session_frame = encode_v2_frame(PT_SESSION_CONFIG, 0, bytes.fromhex(_G_SESSION_PAYLOAD))
    assert session_frame == bytes.fromhex("a5a5020016001d4d") + bytes.fromhex(_G_SESSION_PAYLOAD)
    # 不完整帧 / CRC 破坏
    assert parse_v2_frame(session_frame[:7]) is None
    bad = bytearray(session_frame)
    bad[-1] ^= 0xFF
    try:
        parse_v2_frame(bytes(bad))
        raise AssertionError("CRC 破坏应抛 ValueError")
    except ValueError:
        pass
    # V2Accumulator：多帧拼接、1 字节切块、非法前缀重同步
    f2 = encode_v2_frame(PT_DATA, 3, bytes([0x01, 0x01, 0x08, 0x01]))
    f3 = encode_v2_frame(PT_ACK, 9)
    stream = session_frame + f2 + f3
    acc = V2Accumulator()
    out = []
    for i in range(len(stream)):
        out += acc.feed(stream[i:i + 1])
    assert len(out) == 3 and acc.buf == bytearray(), "1 字节切块应还原全部 3 帧且清空缓冲"
    out = V2Accumulator().feed(b"\xde\xad\xbe\xef" + stream)
    assert len(out) == 3 and out[0][0] == PT_SESSION_CONFIG, "非法前缀应重同步并继续解析"

    # ---- protobuf 编码 golden + 解析往返 ----
    nonce16, mac32 = bytes(16), bytes([0x11]) * 32
    cmd = encode_command_phone_nonce(nonce16)
    assert cmd == bytes.fromhex(_G_PHONE_NONCE_CMD), "PhoneNonce 命令 golden"
    parsed = parse_command(cmd)
    assert parsed["type"] == 1 and parsed["subtype"] == 26 and parsed["phone_nonce"] == nonce16
    assert encode_command_watch_nonce(nonce16, mac32) == bytes.fromhex(_G_WATCH_NONCE_CMD)
    parsed = parse_command(bytes.fromhex(_G_WATCH_NONCE_CMD))
    assert parsed["watch_nonce"] == nonce16 and parsed["watch_hmac"] == mac32
    assert encode_command_auth_step3(nonce16, mac32) == bytes.fromhex(_G_AUTH_STEP3_CMD)
    assert encode_auth_device_info(0, 34.0, b"POC", 224, b"CN") == bytes.fromhex(_G_DEVICE_INFO)

    # ---- derive_session / verify_watch_hmac / phone_ack ----
    secret = bytes.fromhex("00112233445566778899aabbccddeeff")
    pn = bytes.fromhex("000102030405060708090a0b0c0d0e0f")
    wn = bytes.fromhex("101112131415161718191a1b1c1d1e1f")
    derived = derive_session(secret, pn, wn)
    assert len(derived) == 64
    assert derive_session(secret, pn, wn) == derived, "derive_session 确定性"
    dec_key, enc_key, enc_nonce4 = derived[0:16], derived[16:32], derived[36:40]
    wh = hmac_sha256(dec_key, wn + pn)
    assert verify_watch_hmac(dec_key, wn, pn, wh)
    assert not verify_watch_hmac(dec_key, wn, pn, bytes([wh[0] ^ 1]) + wh[1:]), "篡改应失败"
    assert phone_ack(enc_key, pn, wn) == hmac_sha256(enc_key, pn + wn)

    # ---- encrypt_v2（AES-128-CTR，key 即 IV）----
    ctr_key = bytes.fromhex("2b7e151628aed2a6abf7158809cf4f3c")
    ctr_plain = bytes.fromhex("6bc1bee22e409f96e93d7e117393172a") * 2
    assert encrypt_v2(ctr_key, ctr_plain) == bytes.fromhex(_G_CTR_OUT), "CTR 已知向量"
    for n in (0, 1, 16, 31, 100):
        p = os.urandom(n)
        assert decrypt_v2(ctr_key, encrypt_v2(ctr_key, p)) == p

    # ---- encrypt_v1（AES-128-CCM, macBits=32）----
    ccm_key = bytes.fromhex("2b7e151628aed2a6abf7158809cf4f3c")
    ccm_plain = bytes.fromhex("6bc1bee22e409f96e93d7e117393172a")
    # nonce=(encNonce4=deadbeef, 4×0x00, counter=7 u32LE)
    assert encrypt_v1(ccm_key, bytes.fromhex("deadbeef"), 7, ccm_plain) == bytes.fromhex(_G_CCM_OUT)
    # 会话式往返：encrypt_v1(counter=0) → decrypt_v1
    info = bytes.fromhex(_G_DEVICE_INFO)
    assert decrypt_v1(enc_key, enc_nonce4, encrypt_v1(enc_key, enc_nonce4, 0, info)) == info

    # ---- build_auth_frames / parse_auth_response（brief 契约）----
    assert build_auth_frames("0x" + "ab" * 16, [("send", [0x01, 0x02])]) == [b"\x01\x02"]
    assert build_auth_frames("00" * 32, [("send", [0x01, 0x02])]) == [b"\x01\x02"], "send 透传不校验 authkey"
    try:
        build_auth_frames("zz" * 16, [("auth_step3",)])
        raise AssertionError("auth_step3 需合法 authkey")
    except ValueError:
        pass
    pn_fixed, wn_fixed = bytes(16), bytes(16)
    frames = build_auth_frames("ab" * 16, AUTH_FLOW, phone_nonce=pn_fixed, watch_nonce=wn_fixed)
    assert len(frames) == 6
    assert frames[0][0:2] == b"\xa5\xa5" and frames[0][2] == PT_SESSION_CONFIG and frames[1] is None
    assert parse_v2_frame(frames[2])[2][1] == OP_PLAINTEXT
    assert parse_command(parse_v2_frame(frames[2])[2][2:])["phone_nonce"] == pn_fixed
    assert parse_v2_frame(frames[4])[2][1] == OP_PLAINTEXT
    assert frames[3] is None and frames[5] is None

    # parse_auth_response：session_response / watch_nonce（含 HMAC 验证）/ auth_done
    ok, detail = parse_auth_response(
        encode_v2_frame(PT_SESSION_CONFIG, 0, bytes([OP_START_SESSION_RESPONSE])), "session_response")
    assert ok, detail
    derived_f = derive_session(bytes.fromhex("ab" * 16), pn_fixed, wn_fixed)
    wh_f = hmac_sha256(derived_f[0:16], wn_fixed + pn_fixed)
    watch_frame = encode_v2_frame(PT_DATA, 5, bytes([0x01, 0x01]) + encode_command_watch_nonce(wn_fixed, wh_f))
    ok, detail = parse_auth_response(watch_frame, "watch_nonce", authkey_hex="ab" * 16, phone_nonce=pn_fixed)
    assert ok, detail
    ok, _ = parse_auth_response(watch_frame, "watch_nonce", authkey_hex="ab" * 16,
                                phone_nonce=bytes([0x01]) * 16)
    assert not ok, "错误 phone_nonce 应验证失败"
    done_cmd = encode_command(CMD_TYPE_AUTH, CMD_SUBTYPE_CONNECTED)
    done_frame = encode_v2_frame(PT_DATA, 6, bytes([0x01, 0x01]) + done_cmd)
    ok, detail = parse_auth_response(done_frame, "auth_done")
    assert ok, detail

    # ---- authkey 解析 ----
    assert parse_authkey("ab" * 16) == bytes.fromhex("ab" * 16)
    assert parse_authkey("0x" + "ab" * 16) == bytes.fromhex("ab" * 16)
    assert parse_authkey("0X" + "ab" * 16) == bytes.fromhex("ab" * 16)
    assert parse_authkey("zz" * 16) is None and parse_authkey("ab" * 15) is None

    print("self-test OK")


def main() -> None:
    args = sys.argv[1:]
    if "--self-test" in args:
        self_test()
        return
    if "--address" in args and "--authkey" in args:
        address = args[args.index("--address") + 1]
        authkey = args[args.index("--authkey") + 1]
        device_info = default_device_info()
        if "--phone-name" in args:
            device_info["phoneName"] = args[args.index("--phone-name") + 1]
        if "--api-level" in args:
            device_info["phoneApiLevel"] = float(args[args.index("--api-level") + 1])
        if "--region" in args:
            device_info["region"] = args[args.index("--region") + 1].upper()
        try:
            result = asyncio.run(authenticate(address, authkey, device_info))
        except Exception as e:
            result = {"authenticated": False, "detail": f"{type(e).__name__}: {e}"}
        emit_json(result)
        return
    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    main()
