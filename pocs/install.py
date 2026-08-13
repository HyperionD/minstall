"""表盘 bin 解析 + SPP 认证 + WearPacket 表盘安装（MASS 通道分块上传）。

协议来源：docs/protocol-notes.md 第 4、5 节；Band 10 Pro 使用 astrobox 的
WearPacket 协议（认证与推送均确认，见笔记 4.4/4.5/5 节）。

流程（真机验证，2026-08-12）：
  1. SPP 连接 → V1 Hello → V2 START_SESSION → authkey 认证（同 spp_fast.py）
  2. WearPacket{type=WATCH_FACE(4), id=PREPARE_INSTALL(4), prepare_info{id,size,version_code}} → prepare_reply
  3. WearPacket{type=MASS(22), id=PREPARE(0), prepare_request{data_type=16,data_id=md5,data_length}} → prepare_response
  4. MASS 帧上传：L2[channel=2(Mass)][op=1(Write)][total u16][cur u16][fragment]（fragment=slice_length-6）
  5. 等全部 ACK → 完成

用法：
  python pocs/install.py --self-test
  python pocs/install.py --address <addr> --authkey <32hex> --bin <path> [--phone-name X]

stdout 输出一行 JSON：{"ok": bool, "detail": str, "bytes_sent": int}。
"""
import asyncio
import hashlib
import os
import struct
import sys
import time
import zlib

from common import emit_json, log

# ---------------------------------------------------------------------------
# 协议常量（来源见 docs/protocol-notes.md 第 5 节 + astrobox proto）
# ---------------------------------------------------------------------------

# WearPacket Type（wear.proto）
WP_TYPE_WATCH_FACE = 4
WP_TYPE_MASS = 22

# WatchFace WatchFaceID（wear_watch_face.proto）
WF_PREPARE_INSTALL = 4
# Mass MassID（wear_mass.proto）
MASS_PREPARE = 0

# PrepareStatus / InstallResult（wear_common.proto / wear_watch_face.proto）
STATUS_READY = 0

# 表盘类型（MassDataType）
TYPE_WATCHFACE = 16

# 分块（astrobox mass.rs：fragment = slice_length - 6；channel+op+total+cur 共 6B）
DEFAULT_SLICE_LENGTH = 2048   # prepare_response 未给 expected_slice_length 时
MIN_PART_SIZE = 64
L2_OVERHEAD = 6               # channel(1) + op(1) + total(2) + cur(2)

# MASS 负载头（astrobox MassPacket::encode_with_crc32）
FRAME_HEADER = b"\x00"        # comp_data（版本 0）

# ---------------------------------------------------------------------------
# protobuf 编码小工具
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


def encode_wear_packet(pkt_type: int, pkt_id: int, payload_field: int, payload_body: bytes) -> bytes:
    """WearPacket{type=1, id=2, <payload_field>=body}。payload_field 为 3(account)/4(system)/6(watch_face)/7(mass) 等。"""
    return (_field_varint(1, pkt_type)
            + _field_varint(2, pkt_id)
            + _field_bytes(payload_field, payload_body))


def encode_watchface_prepare(watchface_id: str, size: int) -> bytes:
    """WearPacket{type=WATCH_FACE, id=PREPARE_INSTALL, WatchFace{prepare_info{id,size,version_code=65536}}}。"""
    wf = _field_bytes(6, _field_bytes(1, watchface_id.encode()) + _field_varint(2, size)
                      + _field_varint(3, 65536))
    return encode_wear_packet(WP_TYPE_WATCH_FACE, WF_PREPARE_INSTALL, 6, wf)


def encode_mass_prepare(md5: bytes, size: int) -> bytes:
    """WearPacket{type=MASS, id=PREPARE, Mass{prepare_request{data_type=16,data_id=md5,data_length}}}。
    WearPacket payload oneof 中 Mass = field 24（wear.proto）。"""
    req = (_field_varint(1, TYPE_WATCHFACE) + _field_bytes(2, md5) + _field_varint(3, size))
    mass = _field_bytes(1, req)
    return encode_wear_packet(WP_TYPE_MASS, MASS_PREPARE, 24, mass)


def parse_wear_packet(data: bytes) -> dict | None:
    """解析收包 WearPacket。返回 {type, id} + 按 payload 类型提取的字段。"""
    try:
        fields = {n: (w, v) for n, w, v in _parse_fields_raw(data)}
    except ValueError:
        return None
    out = {}
    if isinstance(fields.get(1), tuple) and fields[1][0] == 0:
        out["type"] = fields[1][1]
    if isinstance(fields.get(2), tuple) and fields[2][0] == 0:
        out["id"] = fields[2][1]
    # WatchFace payload（field 6）
    if isinstance(fields.get(6), tuple) and fields[6][0] == 2:
        try:
            wf = {n: v for n, _w, v in _parse_fields_raw(fields[6][1])}
        except ValueError:
            wf = {}
        # prepare_status = field 5（简化响应，真机确认）；prepare_reply = field 9
        if isinstance(wf.get(5), int):
            out["prepare_status"] = wf[5]
        if isinstance(wf.get(9), bytes):
            try:
                pr = {n: v for n, _w, v in _parse_fields_raw(wf[9])}
            except ValueError:
                pr = {}
            if isinstance(pr.get(2), int):
                out["prepare_status"] = pr[2]
            if isinstance(pr.get(4), int):
                out["slice_length"] = pr[4]
            if isinstance(pr.get(1), bytes):
                try:
                    out["reply_id"] = pr[1].decode("utf-8", errors="replace")
                except Exception:
                    pass
    # Mass payload（field 24，wear.proto oneof）
    if isinstance(fields.get(24), tuple) and fields[24][0] == 2:
        try:
            m = {n: v for n, _w, v in _parse_fields_raw(fields[24][1])}
        except ValueError:
            m = {}
        # prepare_response = field 2: {data_id=1, prepare_status=2, expected_slice_length=5}
        if isinstance(m.get(2), bytes):
            try:
                pr = {n: v for n, _w, v in _parse_fields_raw(m[2])}
            except ValueError:
                pr = {}
            if isinstance(pr.get(2), int):
                out["prepare_status"] = pr[2]
            if isinstance(pr.get(5), int):
                out["slice_length"] = pr[5]
            if isinstance(pr.get(1), bytes):
                out["data_id"] = pr[1]
    return out


def _parse_fields_raw(buf: bytes, pos: int = 0):
    """protobuf 字段扫描（与 auth.py 同构，独立实现避免耦合）。"""
    fields = []
    while pos < len(buf):
        tag, pos = _read_varint(buf, pos)
        num, wt = tag >> 3, tag & 7
        if wt == 0:
            v, pos = _read_varint(buf, pos)
            fields.append((num, wt, v))
        elif wt == 2:
            ln, pos = _read_varint(buf, pos)
            fields.append((num, wt, bytes(buf[pos:pos + ln])))
            pos += ln
        else:
            raise ValueError(f"不支持的 wire type {wt}")
    return fields


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


# ---------------------------------------------------------------------------
# Bin 文件解析（XiaomiFWHelper.parseAsWatchface，第 6 节）
# ---------------------------------------------------------------------------


def parse_watchface_id(data: bytes):
    """从表盘 bin 提取 id（offset 0x28 起 null-terminated ASCII，须匹配 ^\\d+$）。"""
    if len(data) < 0x40 or data[0] != 0x5A or data[1] != 0xA5:
        raise ValueError("非法表盘文件：头部 magic 应为 5A A5")
    start = 0x28
    end = data.find(b"\x00", start)
    if end < 0:
        end = min(start + 32, len(data))
    id_bytes = data[start:end]
    try:
        id_str = id_bytes.decode("ascii")
    except UnicodeDecodeError as e:
        raise ValueError(f"表盘 id 非 ASCII: {id_bytes[:16]!r}") from e
    if not id_str.isdigit():
        raise ValueError(f"表盘 id 应为数字: {id_str!r}")
    return id_str


def parse_watchface_name(data: bytes) -> str:
    """提取表盘名称（offset 0x68 起 null-terminated；0x68 处为 0xFFFFFFFF 时走 i18n 表）。"""
    if len(data) < 0x7C:
        return ""
    at = data[0x68:0x6C]
    if at == b"\xff\xff\xff\xff":
        i18n_off = struct.unpack("<I", data[0x74:0x78])[0]
        i18n_size = struct.unpack("<I", data[0x78:0x7C])[0]
        if i18n_off + i18n_size <= len(data):
            tbl = data[i18n_off:i18n_off + i18n_size]
            end = tbl.find(b"\x00")
            if end > 0:
                try:
                    return tbl[:end].decode("utf-8", errors="replace")
                except Exception:
                    pass
        return ""
    end = data.find(b"\x00", 0x68)
    if end < 0:
        end = min(0x68 + 64, len(data))
    try:
        return data[0x68:end].decode("utf-8", errors="replace")
    except Exception:
        return ""


# ---------------------------------------------------------------------------
# MASS 分块（astrobox mass.rs：fragment = slice_length - 6）
# ---------------------------------------------------------------------------


def build_mass_frames(data: bytes, slice_length: int):
    """构造 MASS 分片（纯函数）。返回 (frames, total_parts, with_crc)。

    MassPacket: [comp 1B][type 1B][md5 16B][len u32 LE][bytes] + crc32 u32 LE
    每帧：L2[channel=2][op=1][total u16 LE][cur u16 LE(1 起)][fragment]
    fragment 上限 = slice_length - 6（L2 头 + total/cur 头）
    """
    md5 = hashlib.md5(data).digest()
    framed = FRAME_HEADER + bytes([TYPE_WATCHFACE]) + md5 + struct.pack("<I", len(data)) + data
    with_crc = framed + struct.pack("<I", zlib.crc32(framed) & 0xFFFFFFFF)

    fragment_max = slice_length - L2_OVERHEAD
    if fragment_max < MIN_PART_SIZE:
        fragment_max = MIN_PART_SIZE  # 至少 64，但注意可能超 slice_length（极端场景）
    total_parts = (len(with_crc) + fragment_max - 1) // fragment_max
    frames = []
    for i in range(total_parts):
        fragment = with_crc[i * fragment_max:(i + 1) * fragment_max]
        header = struct.pack("<HH", total_parts, i + 1)  # cur 从 1 起
        frames.append(bytes([2, 1]) + header + fragment)
    return frames, total_parts, with_crc


def chunk_data(data: bytes, size: int):
    """把数据按 size 切块（纯函数，POC 测试契约）。"""
    return [data[i:i + size] for i in range(0, len(data), size)]


# ---------------------------------------------------------------------------
# 自检
# ---------------------------------------------------------------------------


def self_test() -> None:
    # ---- parse_watchface_id / name ----
    fake = bytearray(0x100)
    fake[0] = 0x5A
    fake[1] = 0xA5
    fake[0x28:0x28 + 6] = b"12345\x00"
    assert parse_watchface_id(bytes(fake)) == "12345"
    bad = bytearray(0x100)
    bad[0:2] = b"\x00\x00"
    try:
        parse_watchface_id(bytes(bad))
        raise AssertionError("非法 magic 应抛 ValueError")
    except ValueError:
        pass
    bad2 = bytearray(fake)
    bad2[0x28:0x28 + 5] = b"ab123"
    try:
        parse_watchface_id(bytes(bad2))
        raise AssertionError("非数字 id 应抛 ValueError")
    except ValueError:
        pass
    fake_name = bytearray(0x100)
    fake_name[0x68:0x68 + 7] = b"MyFace\x00"
    assert parse_watchface_name(bytes(fake_name)) == "MyFace"

    # ---- encode_wear_packet / parse_wear_packet ----
    pkt = encode_watchface_prepare("167210067", 2492348)
    assert pkt[0:2] == b"\x08\x04", "type=WATCH_FACE(4)"
    assert pkt[2:4] == b"\x10\x04", "id=PREPARE_INSTALL(4)"
    parsed = parse_wear_packet(pkt)
    assert parsed["type"] == WP_TYPE_WATCH_FACE and parsed["id"] == WF_PREPARE_INSTALL

    mass = encode_mass_prepare(b"\x11" * 16, 100)
    assert mass[0:2] == b"\x08\x16", "type=MASS(22)"
    assert mass[2:4] == b"\x10\x00", "id=PREPARE(0)"
    parsed = parse_wear_packet(mass)
    assert parsed["type"] == WP_TYPE_MASS and parsed["id"] == MASS_PREPARE

    # ---- build_mass_frames ----
    data = b"\x5a\xa5" + b"\x01" * 100
    frames, total, with_crc = build_mass_frames(data, 64)
    assert total == len(frames) and total > 0
    for i, fr in enumerate(frames):
        assert fr[0] == 2 and fr[1] == 1, "channel=2 Mass, op=1 Write"
        t, cur = struct.unpack("<HH", fr[2:6])
        assert t == total and cur == i + 1
    merged = b"".join(fr[6:] for fr in frames)
    assert merged == with_crc
    assert merged[-4:] == struct.pack("<I", zlib.crc32(merged[:-4]) & 0xFFFFFFFF)
    # fragment 不超过上限（仅当 slice_length 足够大时；MIN_PART_SIZE 兜底场景跳过）
    if 64 - 6 >= MIN_PART_SIZE:
        for fr in frames[:-1]:
            assert len(fr[6:]) <= 64 - 6, f"非末块超出 fragment 上限: {len(fr[6:])}"

    # ---- chunk_data ----
    assert chunk_data(b"abcdef", 2) == [b"ab", b"cd", b"ef"]

    print("self-test OK")


# ---------------------------------------------------------------------------
# 主流程（SPP 认证 + WearPacket 安装）
# ---------------------------------------------------------------------------


async def install(address: str, authkey_hex: str, bin_path: str,
                  phone_name: str | None = None) -> dict:
    """SPP 认证后安装表盘。返回 {"ok": bool, "detail": str, "bytes_sent": int}。"""
    from auth import (  # 延迟导入
        V2Accumulator, build_ack_frame, build_protobuf_frame, build_session_config,
        derive_session, encode_auth_device_info, encode_command_auth_step3,
        encode_command_phone_nonce, encrypt_v1, parse_authkey, phone_ack,
        verify_watch_hmac, OP_START_SESSION_REQUEST, PT_DATA, PT_SESSION_CONFIG,
        default_device_info, decrypt_v2, encode_v2_frame,
    )
    from dbus_fast.aio import MessageBus
    from dbus_fast.service import ServiceInterface, method
    from dbus_fast import BusType, Variant

    if not os.path.isfile(bin_path):
        return {"ok": False, "detail": f"文件不存在: {bin_path}", "bytes_sent": 0}
    data = open(bin_path, "rb").read()
    try:
        watchface_id = parse_watchface_id(data)
    except ValueError as e:
        return {"ok": False, "detail": str(e), "bytes_sent": 0}
    log(f"bin: id={watchface_id} name={parse_watchface_name(data)!r} size={len(data)}")

    secret = parse_authkey(authkey_hex)
    if secret is None:
        return {"ok": False, "detail": "authkey 应为 32 hex 字符（可带 0x 前缀）", "bytes_sent": 0}
    di = default_device_info()
    if phone_name:
        di["phoneName"] = phone_name

    # ---- SPP 连接（dbus-fast，同 spp_fast.py）----
    SPP_UUID = "00001101-0000-1000-8000-00805f9b34fb"
    DEV_PATH = "/org/bluez/hci0/dev_" + address.upper().replace(":", "_")
    SPP_HELLO = bytes.fromhex("badcfe00c00300000100ef")

    class SppProfile(ServiceInterface):
        def __init__(self):
            super().__init__("org.bluez.Profile1")
            self.fd = None
            self.connected = asyncio.Event()

        @method()
        def Release(self) -> None:
            pass

        @method()
        def NewConnection(self, device: 'o', fd: 'h', properties: 'a{sv}') -> None:
            self.fd = fd
            self.connected.set()

        @method()
        def RequestDisconnection(self, device: 'o') -> None:
            self.fd = None
            self.connected.clear()

    bus = await MessageBus(bus_type=BusType.SYSTEM, negotiate_unix_fd=True).connect()
    prof = SppProfile()
    bus.export("/com/minstall/spp", prof)
    intro = await bus.introspect("org.bluez", "/org/bluez")
    root = bus.get_proxy_object("org.bluez", "/org/bluez", intro)
    pm = root.get_interface("org.bluez.ProfileManager1")
    opts = {"Role": Variant("s", "client"), "RequireAuthentication": Variant("b", False),
            "RequireAuthorization": Variant("b", False)}
    await pm.call_register_profile("/com/minstall/spp", SPP_UUID, opts)
    dev_intro = await bus.introspect("org.bluez", DEV_PATH)
    dev = bus.get_proxy_object("org.bluez", DEV_PATH, dev_intro)
    di_iface = dev.get_interface("org.bluez.Device1")
    props = dev.get_interface("org.freedesktop.DBus.Properties")
    try:
        connected = (await props.call_get("org.bluez.Device1", "Connected")).value
    except Exception:
        connected = False
    if not connected:
        try:
            await di_iface.call_connect()
        except Exception as e:
            log(f"connect: {type(e).__name__}: {e}")
        await asyncio.sleep(2)
    try:
        await di_iface.call_connect_profile(SPP_UUID)
    except Exception as e:
        log(f"ConnectProfile: {type(e).__name__}: {e}")
    try:
        await asyncio.wait_for(prof.connected.wait(), timeout=10)
    except asyncio.TimeoutError:
        return {"ok": False, "detail": "SPP 连接超时（请确认手环已配对并连接）", "bytes_sent": 0}
    fd = prof.fd
    log(f"SPP 连接建立 fd={fd}")
    os.set_blocking(fd, False)

    def fd_read(n=4096):
        try:
            return os.read(fd, n)
        except BlockingIOError:
            return b""
        except OSError:
            return None

    def write_blocking(data: bytes) -> None:
        import select
        while True:
            try:
                os.write(fd, data)
                return
            except BlockingIOError:
                select.select([], [fd], [], 1.0)

    acc = V2Accumulator()
    enc_key = None
    dec_key = None
    proto_buf = bytearray()

    async def pump(predicate, label, timeout=15.0):
        """收帧直到 predicate 命中。Data 帧回 ACK；PROTOBUF 加密通道先解密；返回谓词参数（("plain", bytes) 等）。"""
        nonlocal enc_key, dec_key, proto_buf
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            data = fd_read()
            if data is None:
                return None
            if data:
                for f in acc.feed(data):
                    pt, seq, payload = f
                    if pt == PT_DATA:
                        try:
                            os.write(fd, build_ack_frame(seq))
                        except OSError:
                            pass
                        if len(payload) >= 2:
                            chan, op = payload[0] & 0x0F, payload[1]
                            body = payload[2:]
                            if chan == 1 and op == 1:
                                proto_buf += body
                                try:
                                    if predicate(("plain", bytes(proto_buf))):
                                        return ("plain", bytes(proto_buf))
                                except TypeError:
                                    pass
                            elif chan == 1 and op == 2 and dec_key:
                                try:
                                    plain = decrypt_v2(dec_key, body)
                                except Exception:
                                    continue
                                try:
                                    if predicate(("enc", plain)):
                                        return ("enc", plain)
                                except TypeError:
                                    pass
                    else:
                        try:
                            if predicate(("frame", f)):
                                return ("frame", f)
                        except TypeError:
                            pass
            await asyncio.sleep(0.05)
        log(f"等待 {label} 超时")
        return None

    # ---- 认证（同 spp_fast.py auth 流程）----
    write_blocking(SPP_HELLO)
    log("→ V1 Hello")
    write_blocking(build_session_config(OP_START_SESSION_REQUEST, seq=0))
    log("→ START_SESSION_REQUEST")
    got = await pump(lambda r: r[0] == "frame" and r[1][0] == PT_SESSION_CONFIG
                     and r[1][2] and r[1][2][0] == 2, "START_SESSION_RESPONSE")
    if got is None:
        return {"ok": False, "detail": "START_SESSION_RESPONSE 超时", "bytes_sent": 0}

    phone_nonce = os.urandom(16)
    write_blocking(build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
    log(f"→ PhoneNonce {phone_nonce.hex()}")
    got = await pump(lambda r: r[0] == "plain" and parse_wear_packet(r[1])
                     and parse_wear_packet(r[1]).get("id") == 26, "WatchNonce", timeout=10)
    if got is None:
        return {"ok": False, "detail": "WatchNonce 超时", "bytes_sent": 0}
    # 用 auth 的 parse_command 提取 watch_nonce（WearPacket 与 Command 字节兼容）
    from auth import parse_command
    watch_pkt = parse_wear_packet(got[1])
    cmd = parse_command(got[1])
    if not cmd or not cmd.get("watch_nonce"):
        return {"ok": False, "detail": "WatchNonce 解析失败", "bytes_sent": 0}
    watch_nonce, watch_hmac = cmd["watch_nonce"], cmd["watch_hmac"]
    derived = derive_session(secret, phone_nonce, watch_nonce)
    dec_key, enc_key = derived[0:16], derived[16:32]
    enc_nonce4 = derived[36:40]
    if not verify_watch_hmac(dec_key, watch_nonce, phone_nonce, watch_hmac):
        return {"ok": False, "detail": "watch HMAC 验证失败（authkey 可能不正确）", "bytes_sent": 0}
    log("watch HMAC 验证通过")

    info_bytes = encode_auth_device_info(
        di["unknown1"], float(di["phoneApiLevel"]),
        str(di["phoneName"]).encode("utf-8"), di["unknown3"],
        str(di["region"]).encode("utf-8"))
    step3 = encode_command_auth_step3(
        phone_ack(enc_key, phone_nonce, watch_nonce),
        encrypt_v1(enc_key, enc_nonce4, 0, info_bytes))
    write_blocking(build_protobuf_frame(1, step3))
    log("→ AuthStep3")
    got = await pump(lambda r: r[0] in ("plain", "enc") and parse_command(r[1])
                     and parse_command(r[1]).get("subtype") in (27, 5), "认证完成", timeout=10)
    if got is None:
        return {"ok": False, "detail": "认证完成应答超时", "bytes_sent": 0}
    log("认证成功")

    # ---- 表盘安装（WearPacket 协议，astrobox 同款）----
    # seq 已用 0(PhoneNonce) 1(AuthStep3)；后续从 2 起
    seq = 2

    def send_enc_wp(wp_bytes: bytes) -> None:
        nonlocal seq
        write_blocking(build_protobuf_frame(seq, wp_bytes, encrypt=True, key=enc_key))
        seq = (seq + 1) & 0xFF

    # 1) WatchFace PrepareInstall
    send_enc_wp(encode_watchface_prepare(watchface_id, len(data)))
    log(f"→ WatchFace PREPARE_INSTALL id={watchface_id} size={len(data)}")
    got = await pump(lambda r: r[0] == "enc" and (wp := parse_wear_packet(r[1]))
                     and wp.get("type") == WP_TYPE_WATCH_FACE and wp.get("id") == WF_PREPARE_INSTALL
                     and "prepare_status" in wp, "watchface prepare_reply", timeout=15)
    if got is None:
        return {"ok": False, "detail": "WatchFace prepare_reply 超时", "bytes_sent": 0}
    wp = parse_wear_packet(got[1])
    log(f"prepare_reply: status={wp.get('prepare_status')} slice={wp.get('slice_length')}")
    if wp.get("prepare_status") != STATUS_READY:
        return {"ok": False, "detail": f"WatchFace prepare_status={wp.get('prepare_status')}（非 READY）", "bytes_sent": 0}

    # 2) Mass Prepare（data_id=md5）
    md5 = hashlib.md5(data).digest()
    send_enc_wp(encode_mass_prepare(md5, len(data)))
    log(f"→ Mass PREPARE type=16 size={len(data)}")
    got = await pump(lambda r: r[0] == "enc" and (wp := parse_wear_packet(r[1]))
                     and wp.get("type") == WP_TYPE_MASS and wp.get("id") == MASS_PREPARE
                     and "prepare_status" in wp, "mass prepare_response", timeout=15)
    if got is None:
        return {"ok": False, "detail": "Mass prepare_response 超时", "bytes_sent": 0}
    wp = parse_wear_packet(got[1])
    log(f"mass prepare: status={wp.get('prepare_status')} slice={wp.get('slice_length')}")
    if wp.get("prepare_status") != STATUS_READY:
        return {"ok": False, "detail": f"Mass prepare_status={wp.get('prepare_status')}（非 READY）", "bytes_sent": 0}
    slice_length = wp.get("slice_length") or DEFAULT_SLICE_LENGTH
    log(f"expected_slice_length={slice_length}")

    # 3) MASS 分片上传（channel=2 Mass, op=1 Write）——批量发送提速
    #    astrobox 参考：batch_limit = tx_window(3) * backlog_multiplier ≈ 18；此处取 8 保守
    frames, total, with_crc = build_mass_frames(data, slice_length)
    data_seq = seq  # 从当前 seq 连续
    BATCH = 2  # 真机验证：>2 时手环断连，2 稳定

    async def drain_until(seq_target: int, timeout: float) -> None:
        """读取直到收到 seq_target 的 ACK（期间回手环推送的 ACK）。"""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            d = fd_read()
            if d:
                for f in acc.feed(d):
                    if f[0] == 1 and f[1] == seq_target:
                        return
                    elif f[0] == PT_DATA:
                        try:
                            os.write(fd, build_ack_frame(f[1]))
                        except OSError:
                            pass
            await asyncio.sleep(0.005)

    idx = 0
    total_bytes = len(data)
    sent_bytes = 0
    t0 = time.monotonic()
    while idx < total:
        # 发一批
        batch_end = min(idx + BATCH, total)
        for j in range(idx, batch_end):
            write_blocking(encode_v2_frame(PT_DATA, data_seq, frames[j]))
            data_seq = (data_seq + 1) & 0xFF
        # 等这批最后一块的 ACK（超时则继续，避免死等）
        last_seq = (data_seq - 1) & 0xFF
        await drain_until(last_seq, 5.0)
        idx = batch_end
        sent_bytes = int(len(with_crc) * idx / total)
        pct = 100.0 * idx / total
        elapsed = time.monotonic() - t0
        speed = sent_bytes / elapsed if elapsed > 0 else 0
        log(f"MASS {idx}/{total} ({pct:.0f}%) {sent_bytes/1024:.0f}KB @ {speed/1024:.0f}KB/s")
    log(f"MASS 上传完成 {total} 块（{len(with_crc)} 字节带帧，{time.monotonic()-t0:.0f}s）")

    # 4) 等手环推送 InstallResult（WatchFace{install_result{id, code}}，code=2=SUCCESS）
    #    astrobox 只等待此推送，无需发额外命令
    install_ok = False
    install_detail = "未收到安装结果"
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        d = fd_read()
        if d:
            for f in acc.feed(d):
                pt, seq, payload = f
                if pt == PT_DATA:
                    try:
                        os.write(fd, build_ack_frame(seq))
                    except OSError:
                        pass
                    if len(payload) >= 2:
                        chan, op = payload[0] & 0x0F, payload[1]
                        body = payload[2:]
                        if chan == 1 and op == 2 and dec_key:
                            try:
                                plain = decrypt_v2(dec_key, body)
                            except Exception:
                                continue
                            wp = parse_wear_packet(plain)
                            if wp and wp.get("type") == WP_TYPE_WATCH_FACE and wp.get("id") == 5:
                                # id=5 = REPORT_INSTALL_RESULT，WatchFace{install_result=7{id=1, code=2}}
                                try:
                                    top = {n: v for n, _w, v in _parse_fields_raw(plain)}
                                    wf = {n: v for n, _w, v in _parse_fields_raw(top.get(6, b""))}
                                    ir = {n: v for n, _w, v in _parse_fields_raw(wf.get(7, b""))}
                                    code = ir.get(2)
                                    install_detail = f"InstallResult code={code}（2=SUCCESS）"
                                    install_ok = (code in (2, 3))  # 2=SUCCESS, 3=INSTALL_USED（已安装，视为成功）
                                    log(f"★ {install_detail}")
                                except Exception as e:
                                    log(f"install_result 解析失败: {e}")
                            elif wp:
                                log(f"  手环推送: type={wp.get('type')} id={wp.get('id')} body={plain.hex()}")
            if install_ok:
                break
        await asyncio.sleep(0.05)
    bus.disconnect()
    if install_ok:
        return {"ok": True, "detail": install_detail, "bytes_sent": len(data)}
    return {"ok": False, "detail": install_detail, "bytes_sent": len(data)}


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    args = sys.argv[1:]
    if "--self-test" in args:
        self_test()
        return
    if "--address" in args and "--authkey" in args and "--bin" in args:
        address = args[args.index("--address") + 1]
        authkey = args[args.index("--authkey") + 1]
        bin_path = args[args.index("--bin") + 1]
        phone_name = None
        if "--phone-name" in args:
            phone_name = args[args.index("--phone-name") + 1]
        try:
            result = asyncio.run(install(address, authkey, bin_path, phone_name))
        except Exception as e:
            result = {"ok": False, "detail": f"{type(e).__name__}: {e}", "bytes_sent": 0}
        emit_json(result)
        return
    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    main()
