package com.minstall.app

import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import org.json.JSONObject

/** Android SAF 文件选择与持久结果邮箱。 */
object BleFilePicker {
    private const val TAG = "BleFilePicker"
    private const val PREFS = "picker_prefs"
    private const val PREFIX_URI = "uri_"
    private const val PREFIX_RESULT = "picker_result_"
    private const val KEY_ACTIVE_REQUEST = "picker_active_request"
    private const val KEY_ACTIVE_STARTED_AT = "picker_active_started_at"
    private const val REQUEST_TIMEOUT_MS = 120_000L
    private const val NO_REQUEST = -1L

    /** 启动选择器后立即返回；结果由 getResult(requestId) 查询。 */
    @JvmStatic
    fun launch(requestId: Long): Boolean {
        val activity = MainActivity.instance
            ?: run { Log.e(TAG, "MainActivity.instance == null"); return false }
        val ctx = AppContext.ctx ?: return false
        synchronized(this) {
            val prefs = getPrefs(ctx)
            if (hasLiveRequest(prefs)) {
                Log.w(TAG, "已有文件选择请求进行中")
                return false
            }
            prefs.edit()
                .putLong(KEY_ACTIVE_REQUEST, requestId)
                .putLong(KEY_ACTIVE_STARTED_AT, System.currentTimeMillis())
                .putString(resultKey(requestId), resultJson("pending"))
                .commit()
        }
        activity.runOnUiThread {
            try {
                activity.launchFilePicker()
            } catch (error: Exception) {
                completeWithError(requestId, "启动文件选择器失败: ${error.message}")
            }
        }
        return true
    }

    /** Activity Result 回调入口。 */
    fun complete(uri: Uri?) {
        val ctx = AppContext.ctx ?: return
        val requestId = getPrefs(ctx).getLong(KEY_ACTIVE_REQUEST, NO_REQUEST)
        if (requestId == NO_REQUEST) {
            Log.w(TAG, "忽略没有对应请求的文件选择结果")
            return
        }
        if (uri == null) {
            saveTerminalResult(ctx, requestId, resultJson("cancelled"))
            return
        }
        try {
            val name = persistAndGetName(ctx, uri)
            saveTerminalResult(ctx, requestId, resultJson("selected", "path", name))
            Log.i(TAG, "已持久授权并记录: $name (requestId=$requestId)")
        } catch (error: Exception) {
            completeWithError(requestId, "保存所选文件失败: ${error.message}")
        }
    }

    /** 返回持久状态；读取不会删除，避免查询响应丢失。 */
    @JvmStatic
    fun getResult(requestId: Long): String {
        val ctx = AppContext.ctx ?: return resultJson("error", "message", "AppContext 未初始化")
        return getPrefs(ctx).getString(resultKey(requestId), null) ?: resultJson("missing")
    }

    /** 前端成功处理结果后确认消费。 */
    @JvmStatic
    fun clearResult(requestId: Long) {
        val ctx = AppContext.ctx ?: return
        synchronized(this) {
            val prefs = getPrefs(ctx)
            val editor = prefs.edit().remove(resultKey(requestId))
            if (prefs.getLong(KEY_ACTIVE_REQUEST, NO_REQUEST) == requestId) {
                editor.remove(KEY_ACTIVE_REQUEST).remove(KEY_ACTIVE_STARTED_AT)
            }
            editor.commit()
        }
    }

    /** 按原始文件名读取持久授权 URI，供 Rust 安装流程使用。 */
    @JvmStatic
    fun readBytes(name: String): ByteArray? {
        val ctx = AppContext.ctx ?: return null
        val uriString = getPrefs(ctx).getString(PREFIX_URI + name, null)
        if (uriString != null) {
            return try {
                ctx.contentResolver.openInputStream(Uri.parse(uriString))?.use { it.readBytes() }
            } catch (error: Exception) {
                Log.w(TAG, "读取 URI 失败($name): ${error.message}")
                null
            }
        }
        return try {
            java.io.File(name).readBytes()
        } catch (error: Exception) {
            Log.w(TAG, "读取文件失败($name): ${error.message}")
            null
        }
    }

    private fun persistAndGetName(ctx: Context, uri: Uri): String {
        ctx.contentResolver.takePersistableUriPermission(
            uri,
            Intent.FLAG_GRANT_READ_URI_PERMISSION,
        )
        val name = queryDisplayName(ctx, uri) ?: "selected_${System.currentTimeMillis()}"
        getPrefs(ctx).edit().putString(PREFIX_URI + name, uri.toString()).commit()
        return name
    }

    private fun queryDisplayName(ctx: Context, uri: Uri): String? {
        return ctx.contentResolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME),
            null,
            null,
            null,
        )?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        }
    }

    private fun completeWithError(requestId: Long, message: String) {
        val ctx = AppContext.ctx ?: return
        Log.e(TAG, "$message (requestId=$requestId)")
        saveTerminalResult(ctx, requestId, resultJson("error", "message", message))
    }

    private fun saveTerminalResult(ctx: Context, requestId: Long, result: String) {
        synchronized(this) {
            getPrefs(ctx).edit()
                .putString(resultKey(requestId), result)
                .remove(KEY_ACTIVE_REQUEST)
                .remove(KEY_ACTIVE_STARTED_AT)
                .commit()
        }
    }

    private fun hasLiveRequest(prefs: SharedPreferences): Boolean {
        val activeRequest = prefs.getLong(KEY_ACTIVE_REQUEST, NO_REQUEST)
        if (activeRequest == NO_REQUEST) return false
        val startedAt = prefs.getLong(KEY_ACTIVE_STARTED_AT, 0L)
        if (System.currentTimeMillis() - startedAt <= REQUEST_TIMEOUT_MS) return true
        prefs.edit()
            .putString(
                resultKey(activeRequest),
                resultJson("error", "message", "文件选择请求已超时"),
            )
            .remove(KEY_ACTIVE_REQUEST)
            .remove(KEY_ACTIVE_STARTED_AT)
            .commit()
        return false
    }

    private fun resultJson(status: String, key: String? = null, value: String? = null): String {
        val json = JSONObject().put("status", status)
        if (key != null) json.put(key, value ?: "")
        return json.toString()
    }

    private fun resultKey(requestId: Long) = PREFIX_RESULT + requestId

    private fun getPrefs(ctx: Context): SharedPreferences =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
