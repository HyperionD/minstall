package com.minstall.app

import android.bluetooth.BluetoothAdapter
import android.util.Log
import java.lang.reflect.Field

/**
 * Android 蓝牙层：RFCOMM（SPP）连接手环，把 socket 的 fd 交给 Rust 协议层。
 *
 * 被 Rust 通过 JNI 调用（见 Rust 侧 Java_com_minstall_app_BleRfcomm_* 入口）：
 *  - connect(addr): 连接 RFCOMM channel（SPP UUID），返回 socket fd；失败返回 -1
 */
object BleRfcomm {
    const val SPP_UUID = "00001101-0000-1000-8000-00805f9b34fb"

    /** 持有当前活跃 socket，防止 GC 关闭 fd（Rust 协议层在用它读写）。 */
    @Volatile
    var activeSocket: android.bluetooth.BluetoothSocket? = null
        private set

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

    /** 关闭当前活跃 socket（fd 归 BluetoothSocket 所有，由 Kotlin 正常 close）。
     *  Rust disconnect 时调用（对应 Rust 侧 AndroidStream::kotlin_close）。 */
    @JvmStatic
    fun close() {
        activeSocket?.let {
            try { it.close() } catch (e: Exception) { Log.w("BleRfcomm", "close: ${e.message}") }
        }
        activeSocket = null
    }

    /**
     * 连接手环 RFCOMM，返回底层 fd（供 Rust tokio 读写）。失败返回 -1。
     * 调用前需已授权 BLUETOOTH_CONNECT 权限。
     *
     * 若连接失败且设备已配对（典型原因：小米运动健康等 App 占用 RFCOMM），
     * 自动「解除配对 → 重新配对 → 重连」：解除配对会强制断开所有现有连接，
     * 相当于"顶掉"占用方。authkey 不受影响（存手环固件，与系统配对无关）。
     */
    @JvmStatic
    fun connect(addr: String): Int {
        return try {
            val adapter = BluetoothAdapter.getDefaultAdapter()
            if (adapter == null) {
                Log.e("BleRfcomm", "adapter==null"); return -1
            }
            if (!adapter.isEnabled) {
                Log.e("BleRfcomm", "adapter not enabled"); return -1
            }
            val device = adapter.getRemoteDevice(addr)
            Log.i("BleRfcomm", "device=${device.name} bondState=${device.bondState}")
            tryConnect(device)
        } catch (e: Exception) {
            Log.e("BleRfcomm", "connect failed: ${e.javaClass.simpleName}: ${e.message}", e)
            -1
        }
    }

    /** 尝试建立 RFCOMM 连接；失败时若已配对则自动重新配对后重试。 */
    private fun tryConnect(device: android.bluetooth.BluetoothDevice): Int {
        val first = openSocket(device)
        if (first >= 0) return first
        // 连接失败：若设备已配对，多半是 RFCOMM 被其它 App 占用
        if (device.bondState == android.bluetooth.BluetoothDevice.BOND_BONDED) {
            Log.w("BleRfcomm", "RFCOMM 连接失败，尝试解除配对顶掉占用方...")
            if (removeBond(device)) {
                // 等待解除完成（官方 App 的连接会被强制断开）
                waitBondState(device, android.bluetooth.BluetoothDevice.BOND_NONE, 5000)
                Thread.sleep(300)
                if (createBond(device)) {
                    Log.i("BleRfcomm", "重新配对中，请在手机/手环确认...")
                    // 等待配对完成（用户确认弹窗，最长 30s）
                    if (waitBondState(device, android.bluetooth.BluetoothDevice.BOND_BONDED, 30000)) {
                        Log.i("BleRfcomm", "重新配对完成，重连...")
                        Thread.sleep(500)
                        return openSocket(device)
                    }
                    Log.e("BleRfcomm", "重新配对超时/未确认")
                }
            }
        }
        return -1
    }

    /** 打开 RFCOMM socket 并返回 fd；失败返回 -1。 */
    private fun openSocket(device: android.bluetooth.BluetoothDevice): Int {
        var socket: android.bluetooth.BluetoothSocket? = null
        return try {
            socket = device.createRfcommSocketToServiceRecord(
                java.util.UUID.fromString(SPP_UUID)
            )
            Log.i("BleRfcomm", "socket created, connecting...")
            socket!!.connect()
            Log.i("BleRfcomm", "socket connected")
            val fd = getSocketFd(socket!!)
            Log.i("BleRfcomm", "fd=$fd")
            if (fd > 0) {
                // 全局持有，防止 GC 关闭 fd；旧 socket 先关
                activeSocket?.let { try { it.close() } catch (_: Exception) {} }
                activeSocket = socket
            }
            fd
        } catch (e: Exception) {
            Log.e("BleRfcomm", "openSocket failed: ${e.javaClass.simpleName}: ${e.message}")
            try { socket?.close() } catch (_: Exception) {}
            -1
        }
    }

    /** 反射调用 removeBond（隐藏 API）：解除配对，强制断开所有现有连接。 */
    private fun removeBond(device: android.bluetooth.BluetoothDevice): Boolean {
        return try {
            val m = device.javaClass.getMethod("removeBond")
            m.invoke(device) as Boolean
        } catch (e: Exception) {
            Log.e("BleRfcomm", "removeBond failed: ${e.message}")
            false
        }
    }

    /** 反射调用 createBond（隐藏 API）：重新配对，触发系统/手环确认。 */
    private fun createBond(device: android.bluetooth.BluetoothDevice): Boolean {
        return try {
            val m = device.javaClass.getMethod("createBond")
            m.invoke(device) as Boolean
        } catch (e: Exception) {
            Log.e("BleRfcomm", "createBond failed: ${e.message}")
            false
        }
    }

    /** 轮询等待 bondState 变为目标状态，超时返回 false。 */
    private fun waitBondState(device: android.bluetooth.BluetoothDevice, target: Int, timeoutMs: Long): Boolean {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            if (device.bondState == target) return true
            Thread.sleep(200)
        }
        return device.bondState == target
    }

    /**
     * 从 BluetoothSocket 提取底层 fd（Android 各版本字段路径不同）：
     *  1. mSocketFd (int)            —— 旧版（≤ API 30 常见）
     *  2. mPfd (ParcelFileDescriptor) —— 部分版本
     *  3. mSocket (LocalSocket) → impl (LocalSocketImpl) → fd (FileDescriptor) —— Android 14+
     * FileDescriptor 内部字段名可能是 descriptor 或 fd。
     */
    private fun getSocketFd(socket: Any): Int {
        val clazz = socket.javaClass
        // 1. mSocketFd (int)
        try {
            val field = clazz.getDeclaredField("mSocketFd")
            field.isAccessible = true
            val v = field.get(socket)
            if (v is Int && v > 0) return v
        } catch (e: Exception) {
            Log.w("BleRfcomm", "mSocketFd: ${e.javaClass.simpleName}: ${e.message}")
        }
        // 2. mPfd (ParcelFileDescriptor → getFd())
        try {
            val field = clazz.getDeclaredField("mPfd")
            field.isAccessible = true
            val v = field.get(socket)
            if (v != null) {
                val m = v.javaClass.getMethod("getFd")
                val fd = m.invoke(v) as Int
                if (fd > 0) return fd
            }
        } catch (e: Exception) {
            Log.w("BleRfcomm", "mPfd: ${e.javaClass.simpleName}: ${e.message}")
        }
        // 3. mSocket → impl → fd (FileDescriptor → descriptor/fd int)
        try {
            val field = clazz.getDeclaredField("mSocket")
            field.isAccessible = true
            val localSocket = field.get(socket) ?: return -1
            val implField = localSocket.javaClass.getDeclaredField("impl")
            implField.isAccessible = true
            val impl = implField.get(localSocket) ?: return -1
            val fdField = impl.javaClass.getDeclaredField("fd")
            fdField.isAccessible = true
            val fileDescriptor = fdField.get(impl) ?: return -1
            for (name in listOf("descriptor", "fd")) {
                try {
                    val df = fileDescriptor.javaClass.getDeclaredField(name)
                    df.isAccessible = true
                    val v = df.getInt(fileDescriptor)
                    if (v > 0) return v
                } catch (e: Exception) {
                    Log.w("BleRfcomm", "FileDescriptor.$name: ${e.javaClass.simpleName}")
                }
            }
        } catch (e: Exception) {
            Log.w("BleRfcomm", "mSocket path: ${e.javaClass.simpleName}: ${e.message}")
        }
        return -1
    }
}
