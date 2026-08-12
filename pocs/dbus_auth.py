"""纯 dbus 认证：捕获手环广播 → dbus Connect → 枚举 GATT → StartNotify(5e) → WriteValue(5f) → V2 认证握手。

背景（2026-08-11 真机结论）：
  - bleak 连接后 ATT 读写报 "Not connected"（BlueZ 后端连接状态与 bleak 报告不一致）
  - dbus 会话内读写正常（read 2a00 OK）
  - 因此认证必须走纯 dbus：同一 dbus 会话完成 connect → 服务发现 → 读写

用法：
  python pocs/dbus_auth.py <authkey>
"""
import asyncio
import os
import struct
import sys
import time

from common import log

try:
    from dbus_next.aio import MessageBus
    from dbus_next import BusType, Variant
    from dbus_next.service import ServiceInterface, method
except ImportError:
    raise SystemExit("缺少 dbus-next：pip install dbus-next")

BAND_ADDR = "2C:0D:CF:73:D9:95"
FE95 = "0000fe95-0000-1000-8000-00805f9b34fb"
V2_TX = "0000005f-0000-1000-8000-00805f9b34fb"
V2_RX = "0000005e-0000-1000-8000-00805f9b34fb"
CCC = "00002902-0000-1000-8000-00805f9b34fb"
DEV_PATH = "/org/bluez/hci0/dev_" + BAND_ADDR.upper().replace(":", "_")


class PairAgent(ServiceInterface):
    """org.bluez.Agent1 NoInputNoOutput（Just Works 配对必需，2026-08-11 真机结论）。"""

    def __init__(self):
        super().__init__("org.bluez.Agent1")

    @method()
    def Release(self):
        pass

    @method()
    def Cancel(self):
        pass

    @method()
    def RequestPinCode(self, device: 's') -> 's':
        return "0000"

    @method()
    def RequestPasskey(self, device: 's') -> 'u':
        return 0

    @method()
    def RequestConfirmation(self, device: 's', passkey: 'u'):
        return None

    @method()
    def RequestAuthorization(self, device: 's'):
        return None

    @method()
    def AuthorizeService(self, device: 's', uuid: 's'):
        return None


async def register_agent(bus):
    bus.export("/com/minstall/agent", PairAgent())
    node = await bus.introspect("org.bluez", "/org/bluez")
    root = bus.get_proxy_object("org.bluez", "/org/bluez", node)
    am = root.get_interface("org.bluez.AgentManager1")
    await am.call_register_agent("/com/minstall/agent", "NoInputNoOutput")
    try:
        await am.call_request_default_agent("/com/minstall/agent")
        log("已注册 NoInputNoOutput agent 并设为默认")
    except Exception as e:
        log(f"设默认 agent 失败（{type(e).__name__}: {e}）")


async def wait_device_ready(bus, timeout=15):
    """轮询等待设备对象出现且接口完整。返回 device proxy 或 None。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            node = await bus.introspect("org.bluez", DEV_PATH)
            dev = bus.get_proxy_object("org.bluez", DEV_PATH, node)
            dev_iface = dev.get_interface("org.bluez.Device1")
            props = dev.get_interface("org.freedesktop.DBus.Properties")
            # 触发一次属性读取，验证接口可用
            _ = await props.call_get("org.bluez.Device1", "Address")
            return dev, dev_iface, props
        except Exception:
            await asyncio.sleep(0.5)
    return None, None, None


async def connect_device(dev_iface, props, timeout=25):
    """dbus Connect 并等待 Connected=True（连接建立后保持，不依赖广播窗口）。"""
    try:
        await dev_iface.call_connect()
    except Exception as e:
        log(f"Connect: {type(e).__name__}: {e}")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            connected = (await props.call_get("org.bluez.Device1", "Connected")).value
            if connected:
                log("Connected=True（连接建立）")
                return True
        except Exception:
            pass
        await asyncio.sleep(0.5)
    log("连接超时（Connected 未变 True）")
    return False


async def wait_services_resolved(props, timeout=25):
    """等待 ServicesResolved=True（服务发现完成）。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            resolved = (await props.call_get("org.bluez.Device1", "ServicesResolved")).value
            if resolved:
                log("ServicesResolved=True")
                return True
        except Exception:
            pass
        await asyncio.sleep(0.5)
    log("服务发现超时（ServicesResolved 未变 True）")
    return False


async def find_characteristics(bus):
    """通过 ObjectManager 枚举 fe95 服务下的特征路径。返回 {uuid: path}。"""
    node = await bus.introspect("org.bluez", "/")
    root = bus.get_proxy_object("org.bluez", "/", node)
    om = root.get_interface("org.freedesktop.DBus.ObjectManager")
    objects = await om.call_get_managed_objects()

    char_paths = {}
    for path, ifaces in objects.items():
        if "org.bluez.GattCharacteristic1" not in ifaces:
            continue
        chprops = {k: v.value for k, v in ifaces["org.bluez.GattCharacteristic1"].items()}
        uuid = chprops.get("UUID", "").lower()
        if uuid in (V2_TX, V2_RX):
            char_paths[uuid] = path
            log(f"特征 {uuid} → {path} props={chprops.get('Flags')}")
    return char_paths


async def get_char(bus, path):
    node = await bus.introspect("org.bluez", path)
    return bus.get_proxy_object("org.bluez", path, node)


async def start_notify(bus, rx_path):
    """写 CCCD=0x0100 启用通知。"""
    node = await bus.introspect("org.bluez", rx_path)
    ch = bus.get_proxy_object("org.bluez", rx_path, node)
    ch_iface = ch.get_interface("org.bluez.GattCharacteristic1")
    # 找 CCCD descriptor
    desc_path = rx_path + "/00002902-0000-1000-8000-00805f9b34fb"
    try:
        dn = await bus.introspect("org.bluez", desc_path)
        desc = bus.get_proxy_object("org.bluez", desc_path, dn)
        d_iface = desc.get_interface("org.bluez.GattDescriptor1")
        await d_iface.call_write_value(bytearray([0x01, 0x00]), {})
        log(f"已写 CCCD=0100 启用通知 ({desc_path})")
    except Exception as e:
        # 备选：StartNotify（BlueZ 会自己写 CCCD）
        log(f"CCCD 写入失败（{type(e).__name__}: {e}），改用 StartNotify")
        try:
            await ch_iface.call_start_notify()
            log("StartNotify OK")
        except Exception as e2:
            log(f"StartNotify 失败: {type(e2).__name__}: {e2}")


async def write_char(bus, tx_path, data):
    node = await bus.introspect("org.bluez", tx_path)
    ch = bus.get_proxy_object("org.bluez", tx_path, node)
    ch_iface = ch.get_interface("org.bluez.GattCharacteristic1")
    await ch_iface.call_write_value(bytearray(data), {"type": Variant("s", "command")})


async def read_value(bus, path):
    node = await bus.introspect("org.bluez", path)
    ch = bus.get_proxy_object("org.bluez", path, node)
    ch_iface = ch.get_interface("org.bluez.GattCharacteristic1")
    return await ch_iface.call_read_value({})


async def run_auth(bus, authkey_hex, rx_path):
    from auth import (
        V2Accumulator, build_ack_frame, build_protobuf_frame, build_session_config,
        derive_session, encode_auth_device_info, encode_command_auth_step3,
        encode_command_phone_nonce, encrypt_v1, parse_authkey, phone_ack,
        verify_watch_hmac, _is_auth_done_cmd, _is_watch_nonce_cmd,
        OP_START_SESSION_REQUEST, PT_DATA, PT_SESSION_CONFIG, default_device_info,
    )
    import os as _os

    secret = parse_authkey(authkey_hex)
    if secret is None:
        return {"ok": False, "detail": "authkey 非法"}

    # 订阅 RX 通知：监听 org.freedesktop.DBus.Properties PropertiesChanged 信号
    rx_events = asyncio.Queue()
    acc = V2Accumulator()

    def on_message(msg):
        if msg.interface != "org.freedesktop.DBus.Properties":
            return
        if msg.member != "PropertiesChanged":
            return
        if msg.path != rx_path:
            return
        # 解析 body: (interface, changed: {name: Variant}, invalidated)
        body = msg.body
        if not body or len(body) < 2:
            return
        changed = body[1]
        if isinstance(changed, dict) and "Value" in changed:
            val = bytes(changed["Value"].value)
            for f in acc.feed(val):
                rx_events.put_nowait(f)

    bus.add_message_handler(on_message)

    await start_notify(bus, rx_path)
    log(f"已启用 {rx_path} 通知")

    async def expect(pred, label, timeout=10.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                f = await asyncio.wait_for(rx_events.get(), timeout=0.5)
            except asyncio.TimeoutError:
                continue
            log(f"RX type={f[0]} seq={f[1]} payload={f[2].hex()}")
            if f[0] == PT_DATA:
                try:
                    await write_char(bus, tx_path, build_ack_frame(f[1]))
                    log(f"→ ACK seq={f[1]}")
                except Exception as e:
                    log(f"ACK 写失败: {e}")
            r = pred(f)
            if r is not None:
                return (r, None)
        return (None, f"等待 {label} 超时")

    # 1) START_SESSION_REQUEST
    await write_char(bus, tx_path, build_session_config(OP_START_SESSION_REQUEST, seq=0))
    log("→ START_SESSION_REQUEST (seq=0)")
    got, err = await expect(
        lambda f: f[0] == PT_SESSION_CONFIG and f[2] and f[2][0] == 2,
        "START_SESSION_RESPONSE")
    if err:
        return {"ok": False, "detail": err}

    # 2) PhoneNonce
    phone_nonce = _os.urandom(16)
    await write_char(bus, tx_path, build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
    log(f"→ PhoneNonce {phone_nonce.hex()}")

    # 3) WatchNonce + HMAC 验证
    got, err = await expect(_is_watch_nonce_cmd, "WatchNonce")
    if err:
        return {"ok": False, "detail": err}
    watch_nonce, watch_hmac = got["watch_nonce"], got["watch_hmac"]
    derived = derive_session(secret, phone_nonce, watch_nonce)
    dec_key, enc_key = derived[0:16], derived[16:32]
    enc_nonce4 = derived[36:40]
    if not verify_watch_hmac(dec_key, watch_nonce, phone_nonce, watch_hmac):
        return {"ok": False, "detail": "watch HMAC 验证失败"}
    log("watch HMAC 验证通过")

    # 4) AuthStep3
    dev = default_device_info()
    info_bytes = encode_auth_device_info(
        dev["unknown1"], float(dev["phoneApiLevel"]),
        str(dev["phoneName"]).encode("utf-8"), dev["unknown3"],
        str(dev["region"]).encode("utf-8"))
    step3 = encode_command_auth_step3(
        phone_ack(enc_key, phone_nonce, watch_nonce),
        encrypt_v1(enc_key, enc_nonce4, 0, info_bytes))
    await write_char(bus, tx_path, build_protobuf_frame(1, step3))
    log("→ AuthStep3")

    # 5) 认证完成
    got, err = await expect(_is_auth_done_cmd, "认证完成应答")
    if err:
        return {"ok": False, "detail": err}
    return {"ok": True, "detail": f"认证成功 (subtype={got['subtype']})"}


async def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    authkey = args[0]

    bus = await MessageBus(bus_type=BusType.SYSTEM).connect()

    # agent 只注册一次（循环外），不占用广播窗口
    await register_agent(bus)

    from bleak import BleakScanner
    log("监听手环广播 ...（请唤醒/重启手环）")
    scanner = BleakScanner()
    await scanner.start()

    while True:
        seen = scanner.discovered_devices_and_advertisement_data
        if BAND_ADDR in seen:
            log(f"★ 捕获广播！立即 dbus connect...")
            # 直接 Connect（设备对象由 bleak 扫描注册），不轮询等待
            for attempt in range(4):
                dev, dev_iface, props = await wait_device_ready(bus, timeout=5)
                if dev is None:
                    log(f"设备对象未出现（attempt {attempt+1}）")
                    continue
                if await connect_device(dev_iface, props, timeout=8):
                    break
                log(f"Connect 失败（attempt {attempt+1}）——快速重试")
                await asyncio.sleep(0.5)
            else:
                log("--- 4 次 Connect 均失败：请再次唤醒/重启手环 ---")
                continue

            if await wait_services_resolved(props, timeout=15):
                # 手环连接保持，读一次 2a00 验证 dbus 读写可用
                char_paths = await find_characteristics(bus)
                if V2_TX not in char_paths or V2_RX not in char_paths:
                    log(f"缺少 V2 特征: {char_paths}")
                    continue
                try:
                    val = await read_value(bus, DEV_PATH + "/service00001800/char00002a00")
                    log(f"dbus 读 2a00 OK: {bytes(val).decode(errors='replace')}")
                except Exception as e:
                    log(f"读 2a00 失败（{type(e).__name__}: {e}）——继续尝试认证")
                result = await run_auth(bus, authkey, char_paths[V2_RX])
                print(f"RESULT: {result}")
                from common import emit_json
                emit_json(result)
                return
            log("--- 重试：请再次唤醒/重启手环 ---")
        await asyncio.sleep(0.3)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
