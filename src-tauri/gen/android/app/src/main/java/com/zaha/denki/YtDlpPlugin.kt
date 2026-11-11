package com.zaha.denki

import android.app.Activity
import android.content.ContentValues
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import com.yausername.youtubedl_android.YoutubeDL
import com.yausername.youtubedl_android.YoutubeDLRequest
import java.io.File
import java.io.FileInputStream
import java.net.URI
import java.util.UUID
import java.util.concurrent.Executors

@InvokeArg
class DownloadArgs {
    lateinit var url: String
}

@TauriPlugin
class YtDlpPlugin(private val activity: Activity) : Plugin(activity) {
    private val executor = Executors.newSingleThreadExecutor()
    private var lastSuccessfulUpdateCheck = 0L
    private var updateDiagnostics = "yt-dlp update has not run"

    @Command
    fun download(invoke: Invoke) {
        val url = try {
            invoke.parseArgs(DownloadArgs::class.java).url.trim()
        } catch (error: Exception) {
            invoke.reject("Enter a valid http or https URL")
            return
        }
        val parsed = try { URI(url) } catch (_: Exception) { null }
        if (parsed == null || (parsed.scheme != "http" && parsed.scheme != "https") || parsed.host.isNullOrBlank()) {
            invoke.reject("Enter a valid http or https URL")
            return
        }

        executor.execute {
            var stagingDir: File? = null
            try {
                YoutubeDL.getInstance().init(activity.applicationContext)
                updateYtDlpIfNeeded()
                val root = File(activity.cacheDir, "ytdlp")
                stagingDir = File(root, UUID.randomUUID().toString())
                if (!stagingDir.mkdirs()) error("Could not create a temporary download folder")

                val request = YoutubeDLRequest(url)
                    .addOption("--no-playlist")
                    .addOption("--no-mtime")
                    .addOption("--no-continue")
                    .addOption("--force-overwrites")
                    .addOption("--no-cache-dir")
                    .addOption("-f", "bestaudio[ext=m4a][acodec^=mp4a]/bestaudio[ext=m4a]/bestaudio[ext=mp3]/bestaudio[ext=flac]/bestaudio[ext=wav]/bestaudio[ext=ogg][acodec^=vorbis]/bestaudio[ext=aac]")
                    .addOption("-o", File(stagingDir, "%(title).200B [%(id)s].%(ext)s").absolutePath)

                YoutubeDL.getInstance().execute(request)
                val downloaded = stagingDir.listFiles()
                    ?.filter { it.isFile && it.length() > 0 }
                    ?.maxByOrNull { it.lastModified() }
                    ?: error("yt-dlp completed without creating an audio file")

                val uri = publishAudio(downloaded)
                invoke.resolveObject(uri.toString())
            } catch (error: Exception) {
                android.util.Log.e("Denki", "Audio download failed", error)
                val details = error.message?.takeIf { it.isNotBlank() } ?: "Audio download failed"
                invoke.reject("$updateDiagnostics\n\n$details")
            } finally {
                stagingDir?.deleteRecursively()
            }
        }
    }

    private fun updateYtDlpIfNeeded() {
        val now = System.currentTimeMillis()
        val oneDayMillis = 24L * 60L * 60L * 1000L
        if (now - lastSuccessfulUpdateCheck < oneDayMillis) return

        try {
            val result = YoutubeDL.getInstance().updateYoutubeDL(
                activity.applicationContext,
                YoutubeDL.UpdateChannel.STABLE
            )
            val version = YoutubeDL.getInstance().versionName(activity.applicationContext) ?: "unknown"
            updateDiagnostics = "yt-dlp update check: $result; installed version: $version"
            android.util.Log.i("Denki", updateDiagnostics)
            lastSuccessfulUpdateCheck = now
        } catch (error: Exception) {
            // Keep older extractors usable offline or when GitHub is temporarily unavailable.
            val version = YoutubeDL.getInstance().versionName(activity.applicationContext) ?: "unknown"
            updateDiagnostics = "yt-dlp update failed; installed version: $version; ${error.message}"
            android.util.Log.w("Denki", "Could not refresh yt-dlp; trying the bundled version", error)
        }
    }

    private fun publishAudio(source: File): String {
        val extension = source.extension.lowercase()
        val mimeType = when (extension) {
            "m4a", "aac" -> "audio/mp4"
            "mp3" -> "audio/mpeg"
            "flac" -> "audio/flac"
            "wav" -> "audio/wav"
            "ogg", "opus" -> "audio/ogg"
            else -> "audio/*"
        }

        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            val musicDir = File(activity.getExternalFilesDir(Environment.DIRECTORY_MUSIC), "Denki Downloads")
            if (!musicDir.exists() && !musicDir.mkdirs()) error("Could not create the music folder")
            val destination = File(musicDir, source.name)
            source.copyTo(destination, overwrite = true)
            return destination.absolutePath
        }

        val values = ContentValues().apply {
            put(MediaStore.Audio.Media.DISPLAY_NAME, source.name)
            put(MediaStore.Audio.Media.MIME_TYPE, mimeType)
            put(MediaStore.Audio.Media.RELATIVE_PATH, "${Environment.DIRECTORY_MUSIC}/Denki Downloads")
            put(MediaStore.Audio.Media.IS_PENDING, 1)
        }
        val resolver = activity.contentResolver
        val uri = resolver.insert(MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, values)
            ?: error("Could not add the download to the Music library")
        try {
            resolver.openOutputStream(uri)?.use { output ->
                FileInputStream(source).use { input -> input.copyTo(output) }
            } ?: error("Could not write the downloaded audio")
            values.clear()
            values.put(MediaStore.Audio.Media.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
            return uri.toString()
        } catch (error: Exception) {
            resolver.delete(uri, null, null)
            throw error
        }
    }
}
