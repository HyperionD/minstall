"""持续监听手环广播 → 自动捕获 → 立即 bluetoothctl 连接+配对 → 打印结果。"""
import asyncio
import subprocess
import time

from bleak import BleakScanner

BAND = '2C:0D:CF:73:D9:95'
LOG = '/tmp/autoconnect.log'


def log(msg):
    line = f'[{time.strftime("%H:%M:%S")}] {msg}'
    print(line, flush=True)
    with open(LOG, 'a') as f:
        f.write(line + '\n')


async def main():
    log('持续监听手环广播 ...（请重启手环，脚本自动连接）')
    scanner = BleakScanner()
    await scanner.start()
    while True:
        seen = scanner.discovered_devices_and_advertisement_data
        if BAND in seen:
            d, adv = seen[BAND]
            log(f'★ 捕获广播 rssi={adv.rssi} —— 执行连接+配对')
            await scanner.stop()
            try:
                cmds = f"agent NoInputNoOutput\npair {BAND}\nconnect {BAND}\nquit\n"
                r = subprocess.run(['bluetoothctl'], input=cmds, capture_output=True,
                                   text=True, timeout=35)
                log(f'--- bluetoothctl 输出 ---\n{r.stdout[-1500:]}')
                r2 = subprocess.run(['bluetoothctl', 'info', BAND], capture_output=True,
                                    text=True, timeout=8)
                log(f'--- info ---\n{r2.stdout[-800:]}')
            except Exception as e:
                log(f'连接异常: {type(e).__name__}: {e}')
            log('--- 本轮结束，重新监听（可再重启手环）---')
            await scanner.start()
        await asyncio.sleep(0.3)


if __name__ == "__main__":
    asyncio.run(main())
