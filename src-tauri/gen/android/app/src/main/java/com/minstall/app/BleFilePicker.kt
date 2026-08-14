package com.minstall.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.util.Log
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Android 文件选择（SAF）：用系统文档选择器选表盘文件，复制到 app 缓存目录，
 * 返回本地路径供 Rust 协议层读取（Rust 只认本地路径，不能直接读 content:// URI）。
 *
 * 被 Rust 通过 JNI 调用（pick），阻塞等待用户选择完成（最长 2 分钟）。
 * 需要在主线程启动 Activity（BleFilePicker 在 JNI 线程执行，通过 runOnUiThread 切主线程）。
 */
object BleFilePicker {
    private const val REQ_PICK = 1002
    private const val TAG = "BleFilePicker"

    /** 由 MainActivity.onActivityResult 回调。 */
    @Volatile
    private var pending = false

    @Volatile
    private var pickResult: String? = null

    private val latch = CountDownLatch(1)

    /**
     * 选择文件并复制到缓存，返回本地绝对路径；取消/失败返回空串。
     * 由 Rust JNI 调用（spawn_blocking 线程）。
     */
    @JvmStatic
    fun pick(): String {
        val activity = MainActivity.instance
            ?: run { Log.e(TAG, "MainActivity.instance == null"); return "" }
        pickResult = null
        pending = true
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
        // 阻塞等待用户选择（onActivityResult 里 countDown）
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
            copyToCache(data.data!!)
        } else {
            "" // 用户取消
        }
        latch.countDown()
    }

    /** 把 content:// URI 复制到 cacheDir，返回本地路径；失败返回 null。 */
    private fun copyToCache(uri: Uri): String? {
        val ctx = AppContext.ctx ?: return null
        return try {
            val name = "watchface_${System.currentTimeMillis()}.bin"
            val outFile = java.io.File(ctx.cacheDir, name)
            ctx.contentResolver.openInputStream(uri)?.use { input ->
                outFile.outputStream().use { output -> input.copyTo(output) }
            } ?: return null
            Log.i(TAG, "copied to ${outFile.absolutePath}")
            outFile.absolutePath
        } catch (e: Exception) {
            Log.e(TAG, "copyToCache failed: $e")
            null
        }
    }
}
