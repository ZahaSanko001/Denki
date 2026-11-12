# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# Tauri loads this plugin by its Android class name at runtime.
-keep class com.zaha.denki.YtDlpPlugin { *; }

# yt-dlp Android uses native and reflective entry points and ships no consumer rules.
-keep class com.yausername.youtubedl_android.** { *; }

# Preserve ZIP metadata helpers used by the yt-dlp Android runtime's update handling.
-keep class org.apache.commons.compress.archivers.zip.** { *; }
