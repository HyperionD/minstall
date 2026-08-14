package com.minstall.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  companion object {
    /** 供 BleFilePicker（JNI 线程）启动 SAF 选择器用。 */
    @Volatile
    var instance: MainActivity? = null
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    instance = this
    AppContext.ctx = applicationContext
    BleRfcomm.init()
    requestBluetoothPermissions()
  }

  /** SAF 文件选择结果转发给 BleFilePicker（JNI 线程在 latch 上等待）。 */
  @Deprecated("Deprecated in Java")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    super.onActivityResult(requestCode, resultCode, data)
    BleFilePicker.onActivityResult(requestCode, resultCode, data)
  }

  /** Android 12+ 需要 BLUETOOTH_CONNECT/SCAN；Android <12 需要位置权限。 */
  private fun requestBluetoothPermissions() {
    val needed = mutableListOf<String>()
    if (Build.VERSION.SDK_INT >= 31) {
      if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_CONNECT)
          != PackageManager.PERMISSION_GRANTED) {
        needed.add(Manifest.permission.BLUETOOTH_CONNECT)
      }
      if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_SCAN)
          != PackageManager.PERMISSION_GRANTED) {
        needed.add(Manifest.permission.BLUETOOTH_SCAN)
      }
    } else {
      if (ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_FINE_LOCATION)
          != PackageManager.PERMISSION_GRANTED) {
        needed.add(Manifest.permission.ACCESS_FINE_LOCATION)
      }
    }
    // 读取导出日志（authkey 提取）；Android 13+ 走 SAF（另处理），此处覆盖 <13
    if (Build.VERSION.SDK_INT < 33) {
      if (ContextCompat.checkSelfPermission(this, Manifest.permission.READ_EXTERNAL_STORAGE)
          != PackageManager.PERMISSION_GRANTED) {
        needed.add(Manifest.permission.READ_EXTERNAL_STORAGE)
      }
    }
    if (needed.isNotEmpty()) {
      ActivityCompat.requestPermissions(this, needed.toTypedArray(), 1001)
    }
  }
}
