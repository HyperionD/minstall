"""对已连接+已配对的手环直接执行 V2 认证（不扫描，复用现有连接）。

用法：python pocs/dbus_auth_direct.py <authkey>
"""
import asyncio
import sys

from dbus_next.aio import MessageBus
from dbus_next import BusType

import dbus_auth as da
from dbus_auth import V2_TX, V2_RX, DEV_PATH
from common import emit_json, log


async def main():
    authkey = sys.argv[1] if len(sys.argv) > 1 else None
    if not authkey:
        print(__doc__)
        sys.exit(2)

    bus = await MessageBus(bus_type=BusType.SYSTEM).connect()

    dev, dev_iface, props = await da.wait_device_ready(bus, timeout=5)
    if dev is None:
        emit_json({"ok": False, "detail": "设备对象不可用（未连接？）"})
        return
    connected = (await props.call_get("org.bluez.Device1", "Connected")).value
    paired = (await props.call_get("org.bluez.Device1", "Paired")).value
    resolved = (await props.call_get("org.bluez.Device1", "ServicesResolved")).value
    log(f"状态: connected={connected} paired={paired} services_resolved={resolved}")

    if not connected:
        log("未连接，尝试 connect...")
        await da.connect_device(dev_iface, props, timeout=10)
        if not (await props.call_get("org.bluez.Device1", "Connected")).value:
            emit_json({"ok": False, "detail": "连接失败"})
            return
        await da.wait_services_resolved(props, timeout=15)

    char_paths, desc_paths = await da.find_characteristics(bus)
    if V2_TX not in char_paths or V2_RX not in char_paths:
        emit_json({"ok": False, "detail": f"缺少 V2 特征: {char_paths}"})
        return
    log(f"TX={char_paths[V2_TX]} RX={char_paths[V2_RX]}")

    result = await da.run_auth(bus, authkey, char_paths[V2_RX], char_paths[V2_TX], desc_paths)
    log(f"认证结果: {result}")
    emit_json(result)


if __name__ == "__main__":
    asyncio.run(main())
