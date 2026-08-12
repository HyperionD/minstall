"""dbus-fast SPP 连接（全部操作在同一 bus）：注册 Profile → 保持存活 → ConnectProfile → 收发。

用法：
  python pocs/spp_fast.py echo <addr>
  python pocs/spp_fast.py auth <addr> <authkey>
"""
import asyncio
import os
import sys
import time

from common import log

from dbus_fast.aio import MessageBus
from dbus_fast.service import ServiceInterface, method
from dbus_fast import BusType, Variant

SPP_UUID = "00001101-0000-1000-8000-00805f9b34fb"
DEV_PATH = "/org/bluez/hci0/dev_2C_0D_CF_73_D9_95"
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
        log(f"NewConnection device={device} fd={fd}")
        self.fd = fd
        self.connected.set()

    @method()
    def RequestDisconnection(self, device: 'o') -> None:
        log("RequestDisconnection")
        self.fd = None
        self.connected.clear()


async def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    cmd, addr = args[0], args[1]

    bus = await MessageBus(bus_type=BusType.SYSTEM, negotiate_unix_fd=True).connect()
    prof = SppProfile()
    bus.export("/com/minstall/spp", prof)

    # 注册 Profile（注册后 bus 保持运行）
    intro = await bus.introspect("org.bluez", "/org/bluez")
    root = bus.get_proxy_object("org.bluez", "/org/bluez", intro)
    pm = root.get_interface("org.bluez.ProfileManager1")
    opts = {
        "Role": Variant("s", "client"),
        "RequireAuthentication": Variant("b", False),
        "RequireAuthorization": Variant("b", False),
    }
    await pm.call_register_profile("/com/minstall/spp", SPP_UUID, opts)
    log("SPP Profile 已注册")

    # 设备 Connect（建 ACL + LE）
    dev_intro = await bus.introspect("org.bluez", DEV_PATH)
    dev = bus.get_proxy_object("org.bluez", DEV_PATH, dev_intro)
    di = dev.get_interface("org.bluez.Device1")
    props = dev.get_interface("org.freedesktop.DBus.Properties")
    try:
        connected = (await props.call_get("org.bluez.Device1", "Connected")).value
    except Exception:
        connected = False
    if not connected:
        log("设备未连接，Connect()...")
        try:
            await di.call_connect()
        except Exception as e:
            log(f"connect: {type(e).__name__}: {e}")
        await asyncio.sleep(2)

    # ConnectProfile（Profile 已注册且存活）
    log("ConnectProfile...")
    try:
        await di.call_connect_profile(SPP_UUID)
        log("ConnectProfile OK")
    except Exception as e:
        log(f"ConnectProfile: {type(e).__name__}: {e}")

    try:
        await asyncio.wait_for(prof.connected.wait(), timeout=10)
        fd = prof.fd
        log(f"SPP 连接建立 fd={fd}")
    except asyncio.TimeoutError:
        log("等待 SPP NewConnection 超时")
        bus.disconnect()
        return

    def fd_read(n=4096):
        try:
            return os.read(fd, n)
        except BlockingIOError:
            return b""
        except OSError:
            return None

    if cmd == "echo":
        os.write(fd, SPP_HELLO)
        log(f"→ Hello {SPP_HELLO.hex()}")
        os.set_blocking(fd, False)
        buf = bytearray()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            data = fd_read()
            if data is None:
                log("连接断开")
                break
            if data:
                buf += data
                log(f"RX {len(data)}B: {data.hex()}")
            await asyncio.sleep(0.1)
        if not buf:
            log("无响应")
        bus.disconnect()
        return

    if cmd == "auth":
        authkey = args[2]
        from auth import (
            V2Accumulator, build_ack_frame, build_protobuf_frame, build_session_config,
            derive_session, encode_auth_device_info, encode_command_auth_step3,
            encode_command_phone_nonce, encrypt_v1, parse_authkey, phone_ack,
            verify_watch_hmac, _is_auth_done_cmd, _is_watch_nonce_cmd,
            OP_START_SESSION_REQUEST, PT_DATA, PT_SESSION_CONFIG, default_device_info,
        )
        import os as _os

        secret = parse_authkey(authkey)
        if secret is None:
            log("authkey 非法")
            bus.disconnect()
            return
        os.set_blocking(fd, False)
        acc = V2Accumulator()
        proto_buf = bytearray()  # 累积 PROTOBUF 通道的 Command 字节

        async def expect(pred, label, timeout=10.0):
            nonlocal proto_buf
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                data = fd_read()
                if data is None:
                    return (None, "连接断开")
                if data:
                    for f in acc.feed(data):
                        log(f"RX type={f[0]} seq={f[1]} payload={f[2].hex()}")
                        if f[0] == PT_DATA:
                            os.write(fd, build_ack_frame(f[1]))
                            log(f"→ ACK seq={f[1]}")
                            # 累积 PROTOBUF 明文通道数据
                            if len(f[2]) >= 2 and (f[2][0] & 0x0F) == 1 and f[2][1] == 1:
                                proto_buf += f[2][2:]
                                log(f"proto_buf += {f[2][2:].hex()}（当前 {len(proto_buf)}B）")
                                # 尝试从累积数据解析 Command
                                from auth import parse_command
                                cmd = parse_command(bytes(proto_buf))
                                if cmd and (cmd.get("watch_nonce") or cmd.get("subtype") in (27, 5)):
                                    log(f"累积解析到 Command: {cmd}")
                                    return (cmd, None)
                        r = pred(f)
                        if r is not None:
                            return (r, None)
                await asyncio.sleep(0.05)
            return (None, f"等待 {label} 超时")

        os.write(fd, SPP_HELLO)
        log("→ V1 Hello")

        os.write(fd, build_session_config(OP_START_SESSION_REQUEST, seq=0))
        log("→ START_SESSION_REQUEST (seq=0)")
        got, err = await expect(
            lambda f: f[0] == 2 and f[2] and f[2][0] == 2, "START_SESSION_RESPONSE")
        if err:
            log(f"失败: {err}")
            bus.disconnect()
            return

        phone_nonce = _os.urandom(16)
        os.write(fd, build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
        log(f"→ PhoneNonce {phone_nonce.hex()}")

        got, err = await expect(_is_watch_nonce_cmd, "WatchNonce")
        if err:
            log(f"失败: {err}")
            bus.disconnect()
            return
        watch_nonce, watch_hmac = got["watch_nonce"], got["watch_hmac"]
        derived = derive_session(secret, phone_nonce, watch_nonce)
        dec_key, enc_key = derived[0:16], derived[16:32]
        enc_nonce4 = derived[36:40]
        if not verify_watch_hmac(dec_key, watch_nonce, phone_nonce, watch_hmac):
            log("watch HMAC 验证失败")
            bus.disconnect()
            return
        log("watch HMAC 验证通过")

        dev_info = default_device_info()
        info_bytes = encode_auth_device_info(
            dev_info["unknown1"], float(dev_info["phoneApiLevel"]),
            str(dev_info["phoneName"]).encode("utf-8"), dev_info["unknown3"],
            str(dev_info["region"]).encode("utf-8"))
        step3 = encode_command_auth_step3(
            phone_ack(enc_key, phone_nonce, watch_nonce),
            encrypt_v1(enc_key, enc_nonce4, 0, info_bytes))
        os.write(fd, build_protobuf_frame(1, step3))
        log("→ AuthStep3")

        got, err = await expect(_is_auth_done_cmd, "认证完成应答")
        if err:
            log(f"失败: {err}")
            bus.disconnect()
            return
        log(f"★★★ SPP 认证成功！★★★ (subtype={got['subtype']})")
        bus.disconnect()
        return

    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
