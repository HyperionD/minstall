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
