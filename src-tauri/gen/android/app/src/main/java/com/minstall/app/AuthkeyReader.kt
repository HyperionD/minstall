package com.minstall.app

import android.content.ClipboardManager
import android.content.Context
import android.os.Environment
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.io.File
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.util.zip.ZipFile
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * authkey 自动读取（全自动，无弹窗）：
 *  1. 剪贴板：用户复制的 32 位 hex
 *  2. Download/wearablelog 下最新导出 zip（自动扫描，无需用户选择）
 *
 * 日志中的提取规则（真机验证 2026-08-14）：手环设备 JSON 的 detail 里
 *   "encryptKey": "32hex"（与 "token" 同值）即 authkey；
 *   "authKey" 字段恒为 null（勿用）。取最后一次出现的 encryptKey（当前绑定）。
 *
 * 返回格式：状态码前缀 + 值（供 Rust 透传给前端判断提示）：
 *   "FOUND|<hex>"   成功
 *   "DIR_MISSING"   目录不存在（引导用户去官方 App 导出日志）
 *   "NEED_PERMISSION" 无存储权限（引导用户开启「所有文件访问」）
 *   "EMPTY"         目录存在但无 zip / 无 authkey
 */
object AuthkeyReader {
    private const val TAG = "AuthkeyReader"
    private const val SECURE_PREFS = "minstall_secure"
    private const val SAVED_CIPHERTEXT = "authkey_ciphertext"
    private const val SAVED_IV = "authkey_iv"
    private const val KEY_ALIAS = "minstall_authkey"
    private const val ANDROID_KEYSTORE = "AndroidKeyStore"

    /** 32 位 hex（允许 0x 前缀）。 */
    private val HEX32 = Regex("(?:0[xX])?[0-9a-fA-F]{32}")

    /** 字段值形态：encryptKey/token 后的 32hex（authKey 恒为 null，不匹配它）。 */
    private val FIELD_KEY = Regex(""""(encryptKey|token)"\s*:\s*"([0-9a-fA-F]{32})"""")

    /** 读取 authkey，返回状态码 + 值。 */
    @JvmStatic
    fun read(): String {
        val ctx = AppContext.ctx
        if (ctx == null) {
            Log.w(TAG, "ctx == null")
            return "EMPTY"
        }
        // 1. 剪贴板
        getClipboardHex(ctx)?.let {
            Log.i(TAG, "从剪贴板读取到 authkey")
            return "FOUND|$it"
        }
        // 2. Download/wearablelog 自动扫描
        return readFromWearableLog(ctx)
    }

    /** 读取 Android Keystore 加密保存的 authkey；未保存或解密失败返回空串。 */
    @JvmStatic
    fun readSaved(): String {
        val ctx = AppContext.ctx ?: return ""
        return try {
            val prefs = ctx.getSharedPreferences(SECURE_PREFS, Context.MODE_PRIVATE)
            val ciphertext = prefs.getString(SAVED_CIPHERTEXT, null) ?: return ""
            val iv = prefs.getString(SAVED_IV, null) ?: return ""
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                getOrCreateKey(),
                GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP))
            )
            val value = String(
                cipher.doFinal(Base64.decode(ciphertext, Base64.NO_WRAP)),
                StandardCharsets.UTF_8
            )
            if (HEX32.matches(value)) value.lowercase() else ""
        } catch (e: Exception) {
            Log.w(TAG, "读取已保存 authkey 失败: ${e.javaClass.simpleName}")
            ""
        }
    }

    /** 使用 Android Keystore 的 AES-GCM 保存 authkey。 */
    @JvmStatic
    fun saveSaved(value: String): Boolean {
        val ctx = AppContext.ctx ?: return false
        return try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
            val ciphertext = cipher.doFinal(value.toByteArray(StandardCharsets.UTF_8))
            val prefs = ctx.getSharedPreferences(SECURE_PREFS, Context.MODE_PRIVATE)
            prefs.edit()
                .putString(SAVED_CIPHERTEXT, Base64.encodeToString(ciphertext, Base64.NO_WRAP))
                .putString(SAVED_IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
                .commit()
        } catch (e: Exception) {
            Log.w(TAG, "保存 authkey 失败: ${e.javaClass.simpleName}")
            false
        }
    }

    /** 清除已保存的 authkey，不删除 Keystore 密钥以便后续重新保存。 */
    @JvmStatic
    fun clearSaved(): Boolean {
        val ctx = AppContext.ctx ?: return false
        return ctx.getSharedPreferences(SECURE_PREFS, Context.MODE_PRIVATE)
            .edit()
            .remove(SAVED_CIPHERTEXT)
            .remove(SAVED_IV)
            .commit()
    }

    private fun getOrCreateKey(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build()
        )
        return generator.generateKey()
    }

    /** 剪贴板中的 32 位 hex；无则 null。 */
    private fun getClipboardHex(ctx: Context): String? {
        return try {
            val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val clip = cm.primaryClip ?: return null
            if (clip.itemCount == 0) return null
            val text = clip.getItemAt(0).coerceToText(ctx)?.toString() ?: return null
            HEX32.find(text.trim())?.value
                ?.removePrefix("0x")?.removePrefix("0X")?.lowercase()
        } catch (e: Exception) {
            Log.w(TAG, "剪贴板读取失败: ${e.message}")
            null
        }
    }

    /** 自动扫描 Download/wearablelog 下最新 zip。 */
    private fun readFromWearableLog(ctx: Context): String {
        // 权限检查：Android 11+ 需要 MANAGE_EXTERNAL_STORAGE（所有文件访问）
        if (android.os.Build.VERSION.SDK_INT >= 30 && !Environment.isExternalStorageManager()) {
            Log.w(TAG, "无 MANAGE_EXTERNAL_STORAGE 权限")
            return "NEED_PERMISSION"
        }
        // Android 10 及以下：READ_EXTERNAL_STORAGE（MainActivity 已请求）
        if (android.os.Build.VERSION.SDK_INT < 30) {
            val granted = ctx.checkSelfPermission(android.Manifest.permission.READ_EXTERNAL_STORAGE)
            if (granted != android.content.pm.PackageManager.PERMISSION_GRANTED) {
                return "NEED_PERMISSION"
            }
        }

        val dir = File(
            Environment.getExternalStorageDirectory(),
            "Download/wearablelog"
        )
        if (!dir.isDirectory) {
            Log.i(TAG, "wearablelog 目录不存在: ${dir.absolutePath}")
            return "DIR_MISSING"
        }
        val zips = dir.listFiles { f -> f.isFile && f.name.endsWith(".zip") }
            ?.sortedByDescending { it.lastModified() }
            ?: emptyList()
        if (zips.isEmpty()) {
            Log.i(TAG, "wearablelog 目录下无 zip")
            return "EMPTY"
        }
        Log.i(TAG, "wearablelog 目录找到 ${zips.size} 个 zip，最新: ${zips.first().name}")
        for (zip in zips) {
            val key = readZipFile(zip) ?: continue
            Log.i(TAG, "${zip.name} 找到 authkey")
            return "FOUND|$key"
        }
        return "EMPTY"
    }

    /** 解析本地 zip 文件，返回最后一次 encryptKey（当前绑定）。 */
    private fun readZipFile(zipFile: File): String? {
        return try {
            ZipFile(zipFile).use { zip ->
                val wanted = listOf("XiaomiFit.main.log", "XiaomiFit.device.log", "Transfer.device.log")
                val ordered = wanted.mapNotNull { w -> zip.getEntry(w) } +
                    zip.entries().asSequence().filter { it.name.endsWith(".log") && it.name !in wanted }
                for (entry in ordered) {
                    if (entry.isDirectory || entry.size > 64 * 1024 * 1024) continue
                    val found = zip.getInputStream(entry).use { s ->
                        scanStream(s.bufferedReader(charset("UTF-8")))
                    }
                    if (found != null) {
                        Log.i(TAG, "zip 内 ${entry.name} 找到 authkey")
                        return found
                    }
                }
                null
            }
        } catch (e: Exception) {
            Log.w(TAG, "zip ${zipFile.name} 解析失败: ${e.message}")
            null
        }
    }

    /** 跳转系统「所有文件访问」设置页（Android 11+）。 */
    @JvmStatic
    fun openStorageSettings() {
        val ctx = AppContext.ctx ?: return
        try {
            val intent = android.content.Intent(
                android.provider.Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                android.net.Uri.parse("package:${ctx.packageName}")
            )
            intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
            ctx.startActivity(intent)
        } catch (e: Exception) {
            Log.w(TAG, "打开存储设置失败: ${e.message}")
            try {
                val intent = android.content.Intent(
                    android.provider.Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION
                )
                intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                ctx.startActivity(intent)
            } catch (e2: Exception) {
                Log.w(TAG, "打开通用存储设置失败: ${e2.message}")
            }
        }
    }

    /** 扫描文本流，返回最后一次出现的 encryptKey/token 值（当前绑定）。 */
    private fun scanStream(input: java.io.Reader): String? {
        var last: String? = null
        input.forEachLine { line ->
            FIELD_KEY.find(line)?.let { m ->
                val v = m.groupValues[2].removePrefix("0x").removePrefix("0X").lowercase()
                if (v != "00000000000000000000000000000000") {
                    last = v
                }
            }
        }
        return last
    }
}
