# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# JNI 桥类：被 Rust native 侧通过字符串引用（loadClass + 静态方法签名）调用，
# R8 无法静态分析到这些引用，必须显式 keep，否则方法被裁剪导致 NoSuchMethodError。
# 注意：Kotlin object 的 @JvmStatic 方法用 `public static ** scan(long)` 写法 R8 可能不匹配，
# 用 { *; } 整体保留最保险。
-keep class com.minstall.app.BleScan { *; }
-keep class com.minstall.app.BleRfcomm { *; }
-keep class com.minstall.app.BleFilePicker { *; }
-keep class com.minstall.app.AuthkeyReader { *; }
-keep class com.minstall.app.AppContext { *; }
