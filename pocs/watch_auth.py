"""捕获手环广播 → 立即执行完整配对+认证流程（含 NoInputNoOutput agent + dbus pair）。

解决手环广播窗口短的问题：捕获到广播后 1 秒内开始配对+连接，不做无谓等待。
复用 auth.py 的全部协议实现。

用法：
  python pocs/watch_auth.py <authkey>
"""
import asyncio
import sys
import time

from bleak import BleakScanner

from common import log

BAND_ADDR = "2C:0D:CF:73:D9:95"


async def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    authkey = args[0]

    from auth import authenticate  # 延迟导入（含完整配对+认证）

    log("持续监听手环广播 ...（请唤醒/重启手环）")
    scanner = BleakScanner()
    await scanner.start()

    while True:
        seen = scanner.discovered_devices_and_advertisement_data
        if BAND_ADDR in seen:
            d, adv = seen[BAND_ADDR]
            log(f"★ 捕获广播 rssi={adv.rssi} —— 立即执行配对+认证...")
            try:
                result = await authenticate(BAND_ADDR, authkey)
                print(f"RESULT: {result}")
                from common import emit_json
                emit_json(result)
                if result.get("authenticated"):
                    log("★★★ 认证成功！★★★")
                    return
                log(f"认证未成功: {result.get('detail')}")
            except Exception as e:
                log(f"认证流程异常: {type(e).__name__}: {e}")
            log("--- 重试：请再次唤醒/重启手环 ---")
        await asyncio.sleep(0.3)


if __name__ == "__main__":
    asyncio.run(main())
