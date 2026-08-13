package com.minstall.app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    AppContext.ctx = applicationContext
    requestBluetoothPermissions()
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
    if (needed.isNotEmpty()) {
      ActivityCompat.requestPermissions(this, needed.toTypedArray(), 1001)
    }
  }
}
