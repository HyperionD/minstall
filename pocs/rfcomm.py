"""通过 libc ctypes 实现 RFCOMM（经典蓝牙 SPP）连接 —— 绕过 Python 无 AF_BLUETOOTH 限制。

用法：
  python pocs/rfcomm.py echo <addr>           # 连接 channel 5 → 发 Hello → 读回
  python pocs/rfcomm.py auth <addr> <authkey> # 连接 channel 5 → V1 Hello → V2 认证
"""
import asyncio
import ctypes
import os
import struct
import sys
import time

from common import log

AF_BLUETOOTH = 31
BTPROTO_RFCOMM = 3
SOCK_STREAM = 1

libc = ctypes.CDLL(None, use_errno=True)
libc.socket.restype = ctypes.c_int
libc.socket.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_int]
libc.close.restype = ctypes.c_int
libc.connect.restype = ctypes.c_int
libc.connect.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]


class SockaddrRC(ctypes.Structure):
    """Linux sockaddr_rc：sa_family_t + bdaddr(6B) + channel(1B)。"""
    _fields_ = [
        ('rc_family', ctypes.c_ushort),
        ('rc_bdaddr', ctypes.c_ubyte * 6),
        ('rc_channel', ctypes.c_ubyte),
    ]


def parse_mac(addr):
    return bytes(int(x, 16) for x in addr.replace("-", ":").split(":"))


def rfcomm_connect(addr, channel=5, timeout=10):
    """通过 libc 创建 RFCOMM socket 并连接。返回 fd。"""
    fd = libc.socket(AF_BLUETOOTH, SOCK_STREAM, BTPROTO_RFCOMM)
    if fd < 0:
        raise OSError(f"socket 失败 fd={fd}")
    sa = SockaddrRC()
    sa.rc_family = AF_BLUETOOTH
    sa.rc_channel = channel
    mac = parse_mac(addr)
    for i, b in enumerate(mac):
        sa.rc_bdaddr[i] = b
    rc = libc.connect(fd, ctypes.byref(sa), ctypes.sizeof(sa))
    if rc != 0:
        errno = ctypes.get_errno()
        libc.close(fd)
        raise OSError(f"connect 失败 errno={errno}: {os.strerror(errno)}")
    return fd


def fd_read(fd, n=4096):
    try:
        return os.read(fd, n)
    except BlockingIOError:
        return b""
    except OSError:
        return None


async def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    cmd, addr = args[0], args[1]

    try:
        fd = rfcomm_connect(addr, channel=5)
        log(f"RFCOMM 连接成功 fd={fd}（channel 5）")
    except OSError as e:
        log(f"RFCOMM 连接失败: {e}")
        sys.exit(1)

    if cmd == "echo":
        hello = bytes.fromhex("badcfe00c00300000100ef")
        os.write(fd, hello)
        log(f"→ Hello {hello.hex()}")
        os.set_blocking(fd, False)
        buf = bytearray()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            data = fd_read(fd)
            if data is None:
                log("连接断开")
                break
            if data:
                buf += data
                log(f"RX {len(data)}B: {data.hex()}")
            await asyncio.sleep(0.1)
        if not buf:
            log("无响应")
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
            return
        os.set_blocking(fd, False)
        acc = V2Accumulator()

        async def expect(pred, label, timeout=10.0):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                data = fd_read(fd)
                if data is None:
                    return (None, "连接断开")
                if data:
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

        # 1) V1 Hello（AstroBox：SPP 连接必发）
        os.write(fd, bytes.fromhex("badcfe00c00300000100ef"))
        log("→ V1 Hello")

        # 2) START_SESSION_REQUEST
        os.write(fd, build_session_config(OP_START_SESSION_REQUEST, seq=0))
        log("→ START_SESSION_REQUEST (seq=0)")
        got, err = await expect(
            lambda f: f[0] == 2 and f[2] and f[2][0] == 2, "START_SESSION_RESPONSE")
        if err:
            log(f"失败: {err}")
            return

        # 3) PhoneNonce
        phone_nonce = _os.urandom(16)
        os.write(fd, build_protobuf_frame(0, encode_command_phone_nonce(phone_nonce)))
        log(f"→ PhoneNonce {phone_nonce.hex()}")

        # 4) WatchNonce
        got, err = await expect(_is_watch_nonce_cmd, "WatchNonce")
        if err:
            log(f"失败: {err}")
            return
        watch_nonce, watch_hmac = got["watch_nonce"], got["watch_hmac"]
        derived = derive_session(secret, phone_nonce, watch_nonce)
        dec_key, enc_key = derived[0:16], derived[16:32]
        enc_nonce4 = derived[36:40]
        if not verify_watch_hmac(dec_key, watch_nonce, phone_nonce, watch_hmac):
            log("watch HMAC 验证失败")
            return
        log("watch HMAC 验证通过")

        # 5) AuthStep3
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

        # 6) 认证完成
        got, err = await expect(_is_auth_done_cmd, "认证完成应答")
        if err:
            log(f"失败: {err}")
            return
        log(f"★★★ SPP 认证成功！★★★ (subtype={got['subtype']})")
        return

    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
