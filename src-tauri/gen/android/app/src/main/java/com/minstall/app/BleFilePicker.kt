package com.minstall.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.net.Uri
import android.util.Log
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Android 文件选择（SAF）：用系统文档选择器选表盘文件。
 *
 * 设计：**不复制到 app 缓存**——对选中的 content:// URI 做持久授权（takePersistableUriPermission），
 * 记录「原始文件名 → URI」映射到 SharedPreferences；安装时按原始文件名经 JNI 直接读 URI 字节流，
 * 因此前端显示/日志保留原始表盘文件名（如 芙宁娜表盘.bin），不产生缓存副本。
 *
 * 被 Rust 通过 JNI 调用（pick / readBytes）。pick 阻塞等待用户选择（最长 2 分钟）。
 */
object BleFilePicker {
    private const val REQ_PICK = 1002
    private const val TAG = "BleFilePicker"
    private const val PREFS = "picker_prefs"
    private const val PREFIX_URI = "uri_"

    @Volatile
    private var pending = false

    @Volatile
    private var pickResult: String? = null

    /** 每次 pick() 新建（countDown 后必须重置，否则第二次 await 立即返回旧值）。 */
    @Volatile
    private var latch: CountDownLatch = CountDownLatch(1)

    /**
     * 选择表盘文件，持久授权 URI，返回**原始文件名**（不含路径）；取消/失败返回空串。
     * 文件名作为 readBytes 的键，映射保存在 SharedPreferences。
     */
    @JvmStatic
    fun pick(): String {
        val activity = MainActivity.instance
            ?: run { Log.e(TAG, "MainActivity.instance == null"); return "" }
        pickResult = null
        pending = true
        latch = CountDownLatch(1)
        activity.runOnUiThread {
            try {
                val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = "*/*"
                    putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("application/octet-stream", "application/x-watchface", "*/*"))
                    putExtra(Intent.EXTRA_ALLOW_MULTIPLE, false)
                }
                activity.startActivityForResult(intent, REQ_PICK)
            } catch (e: Exception) {
                Log.e(TAG, "startActivityForResult failed: $e")
                pickResult = ""
                pending = false
                latch.countDown()
            }
        }
        try {
            latch.await(2, TimeUnit.MINUTES)
        } catch (_: InterruptedException) {}
        pending = false
        return pickResult ?: ""
    }

    /** MainActivity.onActivityResult 回调入口。 */
    @JvmStatic
    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode != REQ_PICK) return
        pickResult = if (resultCode == Activity.RESULT_OK && data?.data != null) {
            persistAndGetName(data.data!!)
        } else {
            "" // 用户取消
        }
        latch.countDown()
    }

    /** 持久授权 URI 并返回原始文件名。 */
    private fun persistAndGetName(uri: Uri): String {
        val ctx = AppContext.ctx ?: return ""
        return try {
            // 持久授权：App 重启后仍可读（用户授权一次）
            ctx.contentResolver.takePersistableUriPermission(
                uri, Intent.FLAG_GRANT_READ_URI_PERMISSION
            )
            val name = queryDisplayName(ctx, uri) ?: "watchface_${System.currentTimeMillis()}.bin"
            // 覆盖同名旧映射
            getPrefs(ctx).edit().putString(PREFIX_URI + name, uri.toString()).apply()
            Log.i(TAG, "已持久授权并记录: $name")
            name
        } catch (e: Exception) {
            Log.e(TAG, "持久授权失败: ${e.message}")
            ""
        }
    }

    /** 查询 SAF URI 的原始文件名。 */
    private fun queryDisplayName(ctx: Context, uri: Uri): String? {
        return try {
            val c = ctx.contentResolver.query(
                uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null
            )
            c?.use {
                if (it.moveToFirst()) {
                    it.getString(0)
                } else null
            }
        } catch (e: Exception) {
            Log.w(TAG, "查询文件名失败: ${e.message}")
            null
        }
    }

    /**
     * 按原始文件名读取表盘字节（Rust JNI 调用）。
     * 优先查持久授权 URI 映射；否则尝试当普通文件路径读取（手动输入场景）。
     * 返回 null 表示失败（读不到）。
     */
    @JvmStatic
    fun readBytes(name: String): ByteArray? {
        val ctx = AppContext.ctx ?: return null
        // 1. SAF 持久授权 URI
        val uriStr = getPrefs(ctx).getString(PREFIX_URI + name, null)
        if (uriStr != null) {
            return try {
                ctx.contentResolver.openInputStream(Uri.parse(uriStr))?.use { it.readBytes() }
            } catch (e: Exception) {
                Log.w(TAG, "读取 URI 失败($name): ${e.message}")
                null
            }
        }
        // 2. 普通文件路径（手动输入）
        return try {
            java.io.File(name).readBytes()
        } catch (e: Exception) {
            Log.w(TAG, "读取文件失败($name): ${e.message}")
            null
        }
    }

    private fun getPrefs(ctx: Context): SharedPreferences =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
