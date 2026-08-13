package com.minstall.app

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * 应用级 Context（由 MainActivity 设置，供蓝牙扫描等需要 Context 的操作使用）。
 */
object AppContext {
    @Volatile
    var ctx: Context? = null
}

/**
 * Android 蓝牙扫描：startDiscovery 发现周边设备。
 * 被 Rust 通过 JNI 调用（BleScan.scanNative），返回 "name|address|rssi" 数组。
 */
object BleScan {

    /**
     * 扫描 timeoutMs 毫秒，返回发现的设备列表（"name|address|rssi"）。
     * 在后台线程执行（内部 CountDownLatch 等待），避免阻塞调用线程。
     */
    @JvmStatic
    fun scan(timeoutMs: Long): Array<String> {
        val adapter = BluetoothAdapter.getDefaultAdapter()
            ?: return emptyArray()
        if (!adapter.isEnabled) return emptyArray()
        val ctx = AppContext.ctx ?: return emptyArray()

        val results = mutableListOf<String>()
        val done = CountDownLatch(1)
        var receiver: BroadcastReceiver? = null

        // 先取消已有发现，再开始新发现
        try { adapter.cancelDiscovery() } catch (_: Exception) {}

        val filter = IntentFilter(BluetoothDevice.ACTION_FOUND)
        filter.addAction(BluetoothAdapter.ACTION_DISCOVERY_FINISHED)
        receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                when (intent.action) {
                    BluetoothDevice.ACTION_FOUND -> {
                        val device: BluetoothDevice? =
                            if (android.os.Build.VERSION.SDK_INT >= 33) {
                                intent.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE, BluetoothDevice::class.java)
                            } else {
                                @Suppress("DEPRECATION")
                                intent.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE)
                            }
                        val rssi = intent.getShortExtra(BluetoothDevice.EXTRA_RSSI, 0)
                        if (device != null) {
                            results.add("${device.name ?: ""}|${device.address}|$rssi")
                        }
                    }
                    BluetoothAdapter.ACTION_DISCOVERY_FINISHED -> done.countDown()
                }
            }
        }
        ctx.registerReceiver(receiver, filter)

        val started = adapter.startDiscovery()
        if (!started) {
            try { ctx.unregisterReceiver(receiver) } catch (_: Exception) {}
            return emptyArray()
        }

        // 等待超时或发现结束
        done.await(timeoutMs, TimeUnit.MILLISECONDS)
        try { adapter.cancelDiscovery() } catch (_: Exception) {}
        try { ctx.unregisterReceiver(receiver) } catch (_: Exception) {}

        return results.toTypedArray()
    }
}
