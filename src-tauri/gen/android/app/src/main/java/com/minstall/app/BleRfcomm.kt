package com.minstall.app

import android.bluetooth.BluetoothAdapter
import java.lang.reflect.Field

/**
 * Android 蓝牙层：RFCOMM（SPP）连接手环，把 socket 的 fd 交给 Rust 协议层。
 *
 * 被 Rust 通过 JNI 调用（见 Rust 侧 Java_com_minstall_app_BleRfcomm_* 入口）：
 *  - connect(addr): 连接 RFCOMM channel（SPP UUID），返回 socket fd；失败返回 -1
 */
object BleRfcomm {
    const val SPP_UUID = "00001101-0000-1000-8000-00805f9b34fb"

    init {
        init()
    }

    /** 显式初始化：加载 native 库并让 Rust 保存 JavaVM（MainActivity 启动时调用）。 */
    @JvmStatic
    fun init() {
        System.loadLibrary("minstall_lib")
        initJni()
    }

    /** 触发 Rust 侧保存 JavaVM（对应 Java_com_minstall_app_BleRfcomm_initJni）。 */
    @JvmStatic
    private external fun initJni()

    /**
     * 连接手环 RFCOMM，返回底层 fd（供 Rust tokio 读写）。失败返回 -1。
     * 调用前需已授权 BLUETOOTH_CONNECT 权限。
     */
    @JvmStatic
    fun connect(addr: String): Int {
        return try {
            val adapter = BluetoothAdapter.getDefaultAdapter()
                ?: return -1
            if (!adapter.isEnabled) return -1
            val device = adapter.getRemoteDevice(addr)
            val socket = device.createRfcommSocketToServiceRecord(
                java.util.UUID.fromString(SPP_UUID)
            )
            socket.connect()
            getSocketFd(socket)
        } catch (e: Exception) {
            -1
        }
    }

    /**
     * 从 BluetoothSocket 提取底层 fd。
     * 优先反射 mSocketFd（各版本通用），失败则尝试 mPfd。
     */
    private fun getSocketFd(socket: Any): Int {
        val clazz = socket.javaClass
        // 尝试 mSocketFd
        for (name in listOf("mSocketFd", "mPfd")) {
            try {
                val field: Field = clazz.getDeclaredField(name)
                field.isAccessible = true
                val v = field.get(socket)
                if (v is Int && v > 0) return v
            } catch (_: Exception) {
                // 继续尝试
            }
        }
        return -1
    }
}
