"""持续监听手环广播并自动连接验证（用于间歇性广播的手环）。

策略：连续扫描，捕获 2C:0D:CF:73:D9:95 后立即尝试 BLE 连接 + GATT 枚举 + 认证。
配合用户手动唤醒/重启手环，最大化捕获窗口。

用法：
  python pocs/watch_listen.py <authkey> [--gatt-only]
"""
import asyncio
import sys
import time

from bleak import BleakScanner, BleakClient

from common import log

BAND_ADDR = "2C:0D:CF:73:D9:95"
FE95 = "0000fe95-0000-1000-8000-00805f9b34fb"
V2_TX = "0000005f-0000-1000-8000-00805f9b34fb"   # write
V2_RX = "0000005e-0000-1000-8000-00805f9b34fb"   # notify


async def gatt_dump(client):
    svc = client.services.get_service(FE95)
    if svc is None:
        return "fe95 服务不存在"
    lines = []
    for c in svc.characteristics:
        lines.append(f"  {c.uuid} props={[p.name for p in c.properties]}")
    return "\n".join(lines)


async def try_auth(client, authkey_hex):
    """最小认证试探：订阅 RX → 写 START_SESSION_REQUEST → 等应答。"""
    from auth import (
        V2Accumulator, build_ack_frame, build_session_config, parse_authkey,
        parse_v2_frame, OP_START_SESSION_REQUEST, PT_DATA, PT_SESSION_CONFIG,
    )
    secret = parse_authkey(authkey_hex)
    if secret is None:
        return "authkey 非法"

    queue = asyncio.Queue()
    acc = V2Accumulator()

    def on_notify(_c, data):
        for f in acc.feed(bytes(data)):
            queue.put_nowait(f)

    svc = client.services.get_service(FE95)
    rx = svc.get_characteristic(V2_RX)
    tx = svc.get_characteristic(V2_TX)
    if rx is None or tx is None:
        return "缺少 V2 RX/TX 特征"

    await client.start_notify(V2_RX, on_notify)
    log("已订阅 V2 RX")

    # 写 START_SESSION_REQUEST
    frame = build_session_config(OP_START_SESSION_REQUEST, seq=0)
    log(f"→ START_SESSION_REQUEST ({len(frame)}B)")
    await client.write_gatt_char(V2_TX, frame, response=False)

    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            f = await asyncio.wait_for(queue.get(), timeout=deadline - time.monotonic())
        except asyncio.TimeoutError:
            return "等待 START_SESSION_RESPONSE 超时（10s 无任何通知）"
        log(f"RX type={f[0]} seq={f[1]} payload={f[2].hex()}")
        if f[0] == PT_DATA:
            await client.write_gatt_char(V2_TX, build_ack_frame(f[1]), response=False)
        if f[0] == PT_SESSION_CONFIG and f[2] and f[2][0] == 2:
            return f"SESSION STARTED: {f[2].hex()}"
    return "超时"


async def main():
    args = sys.argv[1:]
    authkey = args[0] if args else None
    gatt_only = "--gatt-only" in args

    log(f"持续监听 {BAND_ADDR} ...（唤醒/重启手环以触发广播）")
    scanner = BleakScanner()
    await scanner.start()

    attempts = 0
    try:
        while True:
            seen = scanner.discovered_devices_and_advertisement_data
            if BAND_ADDR in seen:
                d, adv = seen[BAND_ADDR]
                log(f"★ 捕获到手环广播！rssi={adv.rssi} —— 尝试连接...")
                await scanner.stop()
                try:
                    async with BleakClient(d, timeout=20) as client:
                        log(f"connected: {client.is_connected}")
                        dump = await gatt_dump(client)
                        log(f"GATT:\n{dump}")
                        if not gatt_only and authkey:
                            result = await try_auth(client, authkey)
                            log(f"认证结果: {result}")
                            if "SESSION STARTED" in result:
                                log("★★★ 认证会话建立成功！★★★")
                                return
                        elif gatt_only:
                            log("GATT 枚举完成（--gatt-only）")
                            return
                except Exception as e:
                    log(f"连接/枚举失败: {type(e).__name__}: {e}")
                attempts += 1
                log(f"--- 尝试 {attempts} 结束，重新监听（可再次唤醒手环）---")
                await scanner.start()
            await asyncio.sleep(0.5)
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    asyncio.run(main())
