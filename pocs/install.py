"""表盘 bin 解析 + SPP 认证 + 加密命令 + DATA 通道分块上传。

协议值来源：docs/protocol-notes.md 第 5、6 节（参考实现 Kodo/Gadgetbridge，部分待真机确认）。
认证与推送共用 SPP 通道（V2 协议，见第 4.3 节）。

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
# 协议常量（来源见 docs/protocol-notes.md 第 5、6 节）
# ---------------------------------------------------------------------------

# 表盘类型 / 命令
TYPE_WATCHFACE = 16
CMD_WATCHFACE_INSTALL = 4      # type=4, subtype=4
CMD_WATCHFACE_SET = 1          # type=4, subtype=1
CMD_DATA_UPLOAD = 0            # type=22, subtype=0
WATCHFACE_ID_FIELD = 2         # Watchface.watchfaceId

# 分块（协议笔记第 5 节，待真机确认）
DEFAULT_CHUNK_SIZE = 2048      # uploadAck 未给 chunkSize 时的默认值
MIN_PART_SIZE = 64
PART_HEADER_SIZE = 4           # [totalParts u16][current u16]
FRAME_HEADER = b"\x00"         # framed 前缀（协议笔记第 5 节）

# ---------------------------------------------------------------------------
# Bin 文件解析（XiaomiFWHelper.parseAsWatchface，第 6 节）
# ---------------------------------------------------------------------------


def parse_watchface_id(data: bytes):
    """从表盘 bin 提取 id（offset 0x28 起 null-terminated ASCII，须匹配 ^\\d+$）。

    返回 id 字符串；非法 bin 抛 ValueError。纯函数，可测试。
    """
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
        # i18n 表：offset 0x74 u32 LE，size 0x78 u32 LE
        i18n_off = struct.unpack("<I", data[0x74:0x78])[0]
        i18n_size = struct.unpack("<I", data[0x78:0x7C])[0]
        if i18n_off + i18n_size <= len(data):
            tbl = data[i18n_off:i18n_off + i18n_size]
            # 取第一条 null-terminated 字符串
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
# 数据上传（第 5 节）
# ---------------------------------------------------------------------------


def build_upload_frames(data: bytes, chunk_size: int):
    """构造 DATA 通道分块帧序列（纯函数）。

    framed = [0x00][type u8][md5 16B][size u32 LE][bytes]
    withCrc = framed + [crc32 u32 LE of framed]
    分块：partSize = chunk_size - 4（至少 64）；每块 = [totalParts u16 LE][current u16 LE][data]

    返回 (frames, total_parts)；frames 每项为一块原始字节。
    """
    md5 = hashlib.md5(data).digest()
    framed = FRAME_HEADER + bytes([TYPE_WATCHFACE]) + md5 + struct.pack("<I", len(data)) + data
    with_crc = framed + struct.pack("<I", zlib.crc32(framed) & 0xFFFFFFFF)

    part_size = max(chunk_size - PART_HEADER_SIZE, MIN_PART_SIZE)
    total_parts = (len(with_crc) + part_size - 1) // part_size
    frames = []
    for i in range(total_parts):
        chunk = with_crc[i * part_size:(i + 1) * part_size]
        header = struct.pack("<HH", total_parts, i)
        frames.append(header + chunk)
    return frames, total_parts


def chunk_data(data: bytes, size: int):
    """把数据按 size 切块（纯函数，POC 测试契约）。"""
    return [data[i:i + size] for i in range(0, len(data), size)]


# ---------------------------------------------------------------------------
# 自检
# ---------------------------------------------------------------------------


def self_test() -> None:
    """纯函数自检：bin 解析 + 分块 + 上传帧构造。"""
    # ---- parse_watchface_id ----
    # 构造合法 bin：magic + id at 0x28
    fake = bytearray(0x100)
    fake[0] = 0x5A
    fake[1] = 0xA5
    fake[0x28:0x28 + 6] = b"12345\x00"
    assert parse_watchface_id(bytes(fake)) == "12345"
    # 非法 magic
    bad = bytearray(0x100)
    bad[0:2] = b"\x00\x00"
    try:
        parse_watchface_id(bytes(bad))
        raise AssertionError("非法 magic 应抛 ValueError")
    except ValueError:
        pass
    # 非数字 id
    bad2 = bytearray(fake)
    bad2[0x28:0x28 + 5] = b"ab123"
    try:
        parse_watchface_id(bytes(bad2))
        raise AssertionError("非数字 id 应抛 ValueError")
    except ValueError:
        pass

    # ---- parse_watchface_name ----
    fake_name = bytearray(0x100)
    fake_name[0x68:0x68 + 7] = b"MyFace\x00"
    assert parse_watchface_name(bytes(fake_name)) == "MyFace"
    # i18n 表路径
    fake_i18n = bytearray(0x200)
    fake_i18n[0x68:0x6C] = b"\xff\xff\xff\xff"
    fake_i18n[0x74:0x78] = struct.pack("<I", 0x100)
    fake_i18n[0x78:0x7C] = struct.pack("<I", 9)  # "i18nName\x00" = 9 字节
    fake_i18n[0x100:0x109] = b"i18nName\x00"
    assert parse_watchface_name(bytes(fake_i18n)) == "i18nName"

    # ---- chunk_data ----
    assert chunk_data(b"abcdef", 2) == [b"ab", b"cd", b"ef"]

    # ---- build_upload_frames ----
    data = b"\x5a\xa5" + b"\x01" * 100
    frames, total = build_upload_frames(data, 64)
    assert total == len(frames)
    assert total > 0
    # 每块头部 [total u16][current u16]
    for i, fr in enumerate(frames):
        t, cur = struct.unpack("<HH", fr[:4])
        assert t == total and cur == i
    # 拼接还原（去掉每块 4B 头 + 4B crc 尾 + framed 头）
    merged = b"".join(fr[4:] for fr in frames)
    assert merged[-4:] == struct.pack("<I", zlib.crc32(merged[:-4]) & 0xFFFFFFFF)
    # crc 验证
    assert zlib.crc32(merged[:-4]) & 0xFFFFFFFF == struct.unpack("<I", merged[-4:])[0]

    print("self-test OK")


# ---------------------------------------------------------------------------
# 主流程（SPP 认证 + 推送）
# ---------------------------------------------------------------------------


async def install(address: str, authkey_hex: str, bin_path: str,
                  phone_name: str | None = None) -> dict:
    """SPP 认证后安装表盘。返回 {"ok": bool, "detail": str, "bytes_sent": int}。"""
    from auth import (  # 延迟导入（bleak/dbus 依赖仅运行时需要）
        V2Accumulator, build_ack_frame, build_protobuf_frame, build_session_config,
        derive_session, encode_auth_device_info, encode_command, encode_command_auth_step3,
        encode_command_phone_nonce, encrypt_v1, parse_authkey, phone_ack,
        verify_watch_hmac, _is_auth_done_cmd, _is_watch_nonce_cmd,
        OP_START_SESSION_REQUEST, PT_DATA, PT_SESSION_CONFIG, default_device_info,
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
    if phone_name:
        di = default_device_info()
        di["phoneName"] = phone_name
    else:
        di = default_device_info()

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

    acc = V2Accumulator()
    enc_key = None
    dec_key = None
    proto_buf = bytearray()

    async def pump(predicate, label, timeout=15.0):
        """收帧直到 predicate 命中；Data 帧回 ACK；加密通道先解密再判定。返回命中帧或 None。"""
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
                                from auth import parse_command
                                cmd = parse_command(bytes(proto_buf))
                                if cmd and predicate(cmd):
                                    return cmd
                            elif chan == 1 and op == 2 and dec_key:
                                from auth import decrypt_v2
                                try:
                                    plain = decrypt_v2(dec_key, body)
                                except Exception:
                                    continue
                                from auth import parse_command
                                cmd = parse_command(plain)
                                if cmd and predicate(cmd):
                                    return cmd
                    else:
                        if predicate(f):
                            return f
            await asyncio.sleep(0.05)
        log(f"等待 {label} 超时")
        return None

    # ---- 认证（同 spp_fast.py auth 流程）----
    os.write(fd, SPP_HELLO)
    log("→ V1 Hello")
    os.write(fd, build_session_config(OP_START_SESSION_REQUEST, seq=0))
    log("→ START_SESSION_REQUEST")
    got = await pump(lambda f: f[0] == PT_SESSION_CONFIG and f[2] and f[2][0] == 2,
                     "START_SESSION_RESPONSE")
    if got is None:
        return {"ok": False, "detail": "START_SESSION_RESPONSE 超时", "bytes_sent": 0}

    phone_nonce = os.urandom(16)
    os.write(fd, build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
    log(f"→ PhoneNonce {phone_nonce.hex()}")
    got = await pump(lambda c: c.get("watch_nonce") is not None, "WatchNonce")
    if got is None:
        return {"ok": False, "detail": "WatchNonce 超时", "bytes_sent": 0}
    watch_nonce, watch_hmac = got["watch_nonce"], got["watch_hmac"]
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
    os.write(fd, build_protobuf_frame(1, step3))
    log("→ AuthStep3")
    got = await pump(lambda c: c.get("subtype") in (27, 5), "认证完成")
    if got is None:
        return {"ok": False, "detail": "认证完成应答超时", "bytes_sent": 0}
    log(f"认证成功 (subtype={got.get('subtype')})")

    # ---- 表盘安装（第 5 节）----
    # 1) CMD_WATCHFACE_INSTALL（加密）
    install_start = _field_bytes(WATCHFACE_ID_FIELD, watchface_id.encode())
    cmd_install = encode_command(4, CMD_WATCHFACE_INSTALL, install_start)
    os.write(fd, build_protobuf_frame(2, cmd_install, encrypt=True, key=enc_key))
    log(f"→ CMD_WATCHFACE_INSTALL id={watchface_id}")
    got = await pump(lambda c: c.get("type") == 4 and c.get("subtype") == 4, "installStatus")
    if got is None:
        return {"ok": False, "detail": "installStatus 应答超时", "bytes_sent": 0}
    status = got.get("install_status")
    if status not in (None, 0):
        return {"ok": False, "detail": f"installStatus={status}（0 才可继续，2=已安装）", "bytes_sent": 0}

    # 2) DATA 上传请求（加密）
    md5 = hashlib.md5(data).digest()
    req_body = _field_varint(1, TYPE_WATCHFACE) + _field_bytes(2, md5) + _field_varint(3, len(data))
    cmd_req = encode_command(22, 0, req_body)
    os.write(fd, build_protobuf_frame(3, cmd_req, encrypt=True, key=enc_key))
    log(f"→ dataUploadRequest size={len(data)}")
    got = await pump(lambda c: c.get("type") == 22 and c.get("subtype") == 0, "dataUploadAck")
    if got is None:
        return {"ok": False, "detail": "dataUploadAck 超时", "bytes_sent": 0}
    chunk_size = got.get("chunk_size") or DEFAULT_CHUNK_SIZE
    resume = got.get("resume_position") or 0
    log(f"dataUploadAck: chunkSize={chunk_size} resume={resume}")

    # 3) DATA 通道分块推送（明文 channel=2）
    frames, total = build_upload_frames(data, chunk_size)
    for idx, fr in enumerate(frames):
        # DATA 通道帧：channel=2, opCode=1（明文），无 V2 protobuf 包装
        payload = bytes([2, 1]) + fr
        from auth import encode_v2_frame
        os.write(fd, encode_v2_frame(PT_DATA, 100 + idx, payload))
        if idx % 16 == 0 or idx == total - 1:
            log(f"DATA {idx + 1}/{total}")
        await asyncio.sleep(0.02)
    log(f"DATA 通道发送完成 {total} 块")

    # 4) 激活（CMD_WATCHFACE_SET）
    set_body = _field_bytes(WATCHFACE_ID_FIELD, watchface_id.encode())
    cmd_set = encode_command(4, CMD_WATCHFACE_SET, set_body)
    os.write(fd, build_protobuf_frame(4, cmd_set, encrypt=True, key=enc_key))
    log("→ CMD_WATCHFACE_SET（激活）")
    await asyncio.sleep(2)  # 给手环处理时间
    bus.disconnect()
    return {"ok": True, "detail": f"已推送 {watchface_id}（{len(data)} 字节，{total} 块）", "bytes_sent": len(data)}


# ---------------------------------------------------------------------------
# protobuf 编码小工具（避免与 auth 内部实现耦合）
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
