"""经典蓝牙 SPP 通道工具（小米手环 10 Pro）。

原理：小米手环（Vela 系统）支持两条独立蓝牙通道：
  - BLE GATT（fe95/5e/5f）—— 手机 App 专用
  - 经典蓝牙 SPP（RFCOMM，UUID 00001101）—— 第三方工具（astrobox 等）专用，免配对

本脚本通过 D-Bus 注册 BlueZ SPP Profile（client 角色），调用 ConnectProfile 建立
RFCOMM 连接，然后以字节流方式收发 V2 协议帧（与 BLE 上的 A5A5 帧格式完全一致）。

用法：
  python pocs/spp.py scan                # BR/EDR 扫描，列出经典蓝牙设备
  python pocs/spp.py probe <addr>        # 查询设备 SDP 服务记录（是否有 SPP）
  python pocs/spp.py echo <addr>         # 连接 SPP → 发 Hello 帧 → 读回字节流
  python pocs/spp.py auth <addr> <authkey>  # 连接 SPP → 完整 V2 认证握手

依赖：dbus-next（pip install dbus-next）
参考：AstroBox btclassic-spp 插件（AGPL-3.0，仅参考连接方式，代码自行编写）
"""
import asyncio
import os
import struct
import sys
import time

from common import emit_json, log

try:
    from dbus_next.aio import MessageBus
    from dbus_next import BusType, Variant
    from dbus_next.service import ServiceInterface, method, signal
except ImportError:
    raise SystemExit("缺少 dbus-next：pip install dbus-next")

SPP_SERVICE_UUID = "00001101-0000-1000-8000-00805f9b34fb"  # 标准 Serial Port Profile

# AstroBox 注释：小米针对 SPP 连接要求先发这个"神秘 Hello"帧
SPP_HELLO = bytes.fromhex("badcfe00c00300000100ef")


class SppProfile(ServiceInterface):
    """org.bluez.Profile1 —— 作为 SPP client 注册，接收 BlueZ 转交的 RFCOMM fd。"""

    def __init__(self, bus):
        super().__init__("org.bluez.Profile1")
        self._bus = bus
        self._fd = None
        self._connected = asyncio.Event()
        self._error = None

    def take_fd(self):
        fd, self._fd = self._fd, None
        return fd

    @method()
    def Release(self):
        pass

    @method()
    def NewConnection(self, device: 'o', fd: 'h', properties: 'a{sv}'):
        """BlueZ 建立 RFCOMM 连接后调用，把 socket fd 交给我们。"""
        log(f"SPP NewConnection device={device} fd={fd}")
        self._fd = fd
        self._connected.set()

    @method()
    def RequestDisconnection(self, device: 'o'):
        log("SPP RequestDisconnection")
        self._fd = None
        self._connected.clear()

    @signal()
    def PropertyChanged(self, name: 's', value: 'v'):
        pass


async def find_adapter(bus):
    root_node = await bus.introspect("org.bluez", "/")
    root = bus.get_proxy_object("org.bluez", "/", root_node)
    mgr = root.get_interface("org.freedesktop.DBus.ObjectManager")
    objects = await mgr.call_get_managed_objects()
    adapter_path = None
    for path, ifaces in objects.items():
        if "org.bluez.Adapter1" in ifaces:
            adapter_path = path
            break
    if adapter_path is None:
        raise RuntimeError("未找到蓝牙适配器")
    return adapter_path


async def register_spp_profile(bus, profile_path):
    """注册 SPP Profile（client，免认证免授权 —— 与 AstroBox 一致）。"""
    root_node = await bus.introspect("org.bluez", "/org/bluez")
    root = bus.get_proxy_object("org.bluez", "/org/bluez", root_node)
    pm = root.get_interface("org.bluez.ProfileManager1")
    options = {
        "Role": Variant("s", "client"),
        "RequireAuthentication": Variant("b", False),
        "RequireAuthorization": Variant("b", False),
    }
    await pm.call_register_profile(profile_path, SPP_SERVICE_UUID, options)
    log(f"已注册 SPP Profile {SPP_SERVICE_UUID}")


async def connect_spp(bus, profile, adapter_path, address):
    """ConnectProfile 建立 RFCOMM 连接，返回 fd（或 None）。"""
    dev_path = adapter_path + "/dev_" + address.upper().replace(":", "_")
    try:
        node = await bus.introspect("org.bluez", dev_path)
    except Exception as e:
        log(f"设备 {address} 未在 BlueZ 中注册（{type(e).__name__}）——先扫描或手动 connect")
        return None
    dev = bus.get_proxy_object("org.bluez", dev_path, node)
    dev_iface = dev.get_interface("org.bluez.Device1")
    try:
        await dev_iface.call_connect_profile(SPP_SERVICE_UUID)
    except Exception as e:
        log(f"ConnectProfile 失败: {type(e).__name__}: {e}")
        return None
    try:
        await asyncio.wait_for(profile._connected.wait(), timeout=10)
    except asyncio.TimeoutError:
        log("等待 SPP NewConnection 超时")
        return None
    fd = profile.take_fd()
    if fd is None:
        log("NewConnection 未提供有效 fd")
        return None
    log(f"SPP 连接成功 fd={fd}")
    return fd


def fd_read(fd, n=4096):
    try:
        return os.read(fd, n)
    except BlockingIOError:
        return b""
    except OSError as e:
        return None  # 连接断开


async def read_until(fd, predicate, timeout=10.0, quiet=True):
    """从 fd 读字节流直到 predicate 命中。返回累积 buffer。"""
    deadline = time.monotonic() + timeout
    buf = bytearray()
    os.set_blocking(fd, False)
    while time.monotonic() < deadline:
        data = fd_read(fd)
        if data is None:
            return (None, "连接断开")
        if data:
            buf += data
            if not quiet:
                log(f"RX {len(data)}B: {data.hex()}")
            if predicate(buf):
                return (bytes(buf), None)
        await asyncio.sleep(0.05)
    return (bytes(buf), "超时")


async def scan_bredr(bus, timeout=10.0):
    """BR/EDR 扫描（经典蓝牙可发现设备）。"""
    adapter_path = await find_adapter(bus)
    node = await bus.introspect("org.bluez", adapter_path)
    adapter = bus.get_proxy_object("org.bluez", adapter_path, node)
    iface = adapter.get_interface("org.bluez.Adapter1")
    # 确保 powered
    props = adapter.get_interface("org.freedesktop.DBus.Properties")
    try:
        await props.call_set("org.bluez.Adapter1", "Powered", Variant("b", True))
    except Exception:
        pass
    # 经典蓝牙扫描：StartDiscovery 会同时做 BR/EDR + LE（取决于内核）
    log(f"开始扫描 {timeout}s ...")
    await iface.call_set_discovery_filter({"Transport": Variant("s", "bredr")})
    try:
        await iface.call_start_discovery()
    except Exception as e:
        log(f"StartDiscovery: {type(e).__name__}: {e}")
    await asyncio.sleep(timeout)
    try:
        await iface.call_stop_discovery()
    except Exception:
        pass

    mgr_node = await bus.introspect("org.bluez", "/")
    mgr = bus.get_proxy_object("org.bluez", "/", mgr_node).get_interface("org.freedesktop.DBus.ObjectManager")
    objects = await mgr.call_get_managed_objects()
    devices = []
    for path, ifaces in objects.items():
        if "org.bluez.Device1" not in ifaces:
            continue
        props = {k: v.value for k, v in ifaces["org.bluez.Device1"].items()}
        if not props.get("Address"):
            continue
        name = props.get("Alias") or props.get("Name") or ""
        devices.append({
            "address": props["Address"],
            "name": name,
            "paired": props.get("Paired", False),
            "connected": props.get("Connected", False),
        })
    # 只显示 BR/EDR 相关（Address 非 random 且非 LE 广告地址的粗略过滤：看有没有 SDP 记录）
    return devices


async def probe_sdp(bus, address):
    """查询设备 SDP 记录（设备需已 connect 或可发现）。"""
    adapter_path = await find_adapter(bus)
    dev_path = adapter_path + "/dev_" + address.upper().replace(":", "_")
    try:
        node = await bus.introspect("org.bluez", dev_path)
    except Exception as e:
        return {"error": f"设备未注册: {e}"}
    dev = bus.get_proxy_object("org.bluez", dev_path, node)
    iface = dev.get_interface("org.bluez.Device1")
    uuids = []
    try:
        props = dev.get_interface("org.freedesktop.DBus.Properties")
        uuids = (await props.call_get("org.bluez.Device1", "UUIDs")).value or []
    except Exception as e:
        return {"error": f"读取 UUIDs 失败: {e}"}
    return {"address": address, "uuids": uuids}


async def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)

    bus = await MessageBus(bus_type=BusType.SYSTEM, negotiate_unix_fd=True).connect()
    profile = SppProfile(bus)

    cmd = args[0]

    if cmd == "scan":
        devices = await scan_bredr(bus, timeout=float(args[1]) if len(args) > 1 else 10.0)
        emit_json({"devices": devices})
        for d in devices:
            print(f"  {d['name'] or '(unnamed)'}  {d['address']}  paired={d['paired']} connected={d['connected']}")
        return

    if cmd == "probe":
        address = args[1]
        emit_json(await probe_sdp(bus, address))
        return

    if cmd == "echo":
        address = args[1]
        await register_spp_profile(bus, "/com/minstall/spp")
        fd = await connect_spp(bus, profile, await find_adapter(bus), address)
        if fd is None:
            emit_json({"ok": False, "detail": "SPP 连接失败"})
            return
        # 发 Hello 帧，看手环是否回数据
        os.write(fd, SPP_HELLO)
        log(f"→ Hello {SPP_HELLO.hex()}")
        buf, err = await read_until(fd, lambda b: len(b) > 0, timeout=5.0, quiet=False)
        os.close(fd)
        if err:
            emit_json({"ok": False, "detail": f"无响应: {err}"})
        else:
            emit_json({"ok": True, "detail": f"收到 {buf.hex()}"})
        return

    if cmd == "auth":
        # SPP 完整认证握手：复用 auth.py 的 V2 帧/加密算法
        from auth import (
            AUTH_FLOW, V2Accumulator, build_ack_frame, build_auth_frames,
            build_protobuf_frame, build_session_config, derive_session, encode_auth_device_info,
            encode_command_auth_step3, encode_command_phone_nonce, encrypt_v1, os as _os,
            parse_authkey, parse_v2_frame, phone_ack, verify_watch_hmac,
            OP_START_SESSION_REQUEST, PT_DATA, PT_SESSION_CONFIG,
        )
        address = args[1]
        authkey = args[2]
        secret = parse_authkey(authkey)
        if secret is None:
            emit_json({"ok": False, "detail": "authkey 应为 32 hex 字符"})
            return
        await register_spp_profile(bus, "/com/minstall/spp")
        fd = await connect_spp(bus, profile, await find_adapter(bus), address)
        if fd is None:
            emit_json({"ok": False, "detail": "SPP 连接失败"})
            return
        os.set_blocking(fd, False)
        acc = V2Accumulator()

        async def flush_read(secs=0.3):
            """把连接后手环主动推送的帧收干净。"""
            await asyncio.sleep(secs)
            while True:
                data = fd_read(fd)
                if not data:
                    break
                for f in acc.feed(data):
                    log(f"RX(flush) type={f[0]} seq={f[1]} payload={f[2].hex()}")

        async def expect(pred, label, timeout=10.0):
            """收帧直到 pred 命中；Data 帧回 ACK；返回 (帧, 错误)。"""
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                while True:
                    data = fd_read(fd)
                    if not data:
                        break
                    for f in acc.feed(data):
                        log(f"RX type={f[0]} seq={f[1]} payload={f[2].hex()}")
                        if f[0] == PT_DATA:
                            os.write(fd, build_ack_frame(f[1]))
                            log(f"→ ACK seq={f[1]}")
                        r = pred(f)
                        if r is not None:
                            return (r, None)
                await asyncio.sleep(0.05)
            return (None, f"等待 {label} 超时")

        # 1) Hello 帧（AstroBox 注释：小米 SPP 连接要求先发）
        os.write(fd, SPP_HELLO)
        log(f"→ Hello {SPP_HELLO.hex()}")
        await flush_read()

        # 2) START_SESSION_REQUEST
        os.write(fd, build_session_config(OP_START_SESSION_REQUEST, seq=0))
        log("→ START_SESSION_REQUEST (seq=0)")
        got, err = await expect(
            lambda f: f[0] == PT_SESSION_CONFIG and f[2] and f[2][0] == 2,
            "START_SESSION_RESPONSE")
        if err:
            emit_json({"ok": False, "detail": err})
            return

        # 3) PhoneNonce 明文
        phone_nonce = _os.urandom(16)
        os.write(fd, build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
        log(f"→ PhoneNonce {phone_nonce.hex()}")

        # 4) 等 WatchNonce
        from auth import _is_watch_nonce_cmd
        got, err = await expect(_is_watch_nonce_cmd, "WatchNonce")
        if err:
            emit_json({"ok": False, "detail": err})
            return
        watch_nonce, watch_hmac = got["watch_nonce"], got["watch_hmac"]
        derived = derive_session(secret, phone_nonce, watch_nonce)
        dec_key, enc_key = derived[0:16], derived[16:32]
        enc_nonce4 = derived[36:40]
        if not verify_watch_hmac(dec_key, watch_nonce, phone_nonce, watch_hmac):
            emit_json({"ok": False, "detail": "watch HMAC 验证失败"})
            return
        log("watch HMAC 验证通过")

        # 5) AuthStep3
        from auth import default_device_info
        dev = default_device_info()
        info_bytes = encode_auth_device_info(
            dev["unknown1"], float(dev["phoneApiLevel"]),
            str(dev["phoneName"]).encode("utf-8"), dev["unknown3"],
            str(dev["region"]).encode("utf-8"))
        step3 = encode_command_auth_step3(
            phone_ack(enc_key, phone_nonce, watch_nonce),
            encrypt_v1(enc_key, enc_nonce4, 0, info_bytes))
        os.write(fd, build_protobuf_frame(1, step3))
        log("→ AuthStep3")

        # 6) 等认证完成
        from auth import _is_auth_done_cmd
        got, err = await expect(_is_auth_done_cmd, "认证完成应答")
        if err:
            emit_json({"ok": False, "detail": err})
            return
        emit_json({"ok": True, "detail": f"SPP 认证成功 (subtype={got['subtype']})"})
        return

    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
