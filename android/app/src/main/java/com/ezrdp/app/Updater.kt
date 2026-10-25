package com.ezrdp.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.widget.Toast
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * Updates the app from the newest build on GitHub. The APK's sha256 must match the update list,
 * and Android itself only accepts it if it's signed with the same key as this app.
 *
 * On Android 12 and later, once "install unknown apps" is allowed for EzRDP, updates install
 * without a prompt; older phones show Android's "Update this app?" screen. Installing closes
 * the app; [Restarted] opens it again afterwards.
 */
object Updater {
    private const val DOWNLOADS = "https://github.com/NeedMeSomeAnimeTiddy/EzRDP/releases/latest/download/"
    private const val CHANNEL = "updates"
    /** Automatic checks at most this often. */
    private const val CHECK_EVERY_MS = 6 * 3600_000L

    class Latest(val build: Long, val file: String, val sha256: String)

    @Volatile private var busy = false

    fun currentBuild(context: Context): Long {
        val info = context.packageManager.getPackageInfo(context.packageName, 0)
        @Suppress("DEPRECATION")
        return if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()
    }

    private fun get(url: String): HttpURLConnection {
        val c = URL(url).openConnection() as HttpURLConnection
        c.connectTimeout = 15_000
        c.readTimeout = 30_000
        c.instanceFollowRedirects = true
        if (c.responseCode != 200) throw IOException("GitHub answered ${c.responseCode}")
        return c
    }

    /** The newest build if it's newer than this app, else null. Network: call off the main thread. */
    fun check(context: Context): Latest? {
        val json = JSONObject(get(DOWNLOADS + "latest.json").inputStream.bufferedReader().use { it.readText() })
        val a = json.getJSONObject("android")
        val latest = Latest(json.getLong("build"), a.getString("file"), a.getString("sha256").lowercase())
        return if (latest.build > currentBuild(context)) latest else null
    }

    /** Whether Android lets EzRDP install updates (Settings → Install unknown apps). */
    fun allowed(context: Context) = context.packageManager.canRequestPackageInstalls()

    /** Opens the screen where the person allows EzRDP to install updates. */
    fun askPermission(context: Context) {
        context.startActivity(
            Intent(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        )
    }

    /** Downloads, checks and installs [latest]. Network: call off the main thread. */
    fun install(context: Context, latest: Latest) {
        if (busy) throw IOException("An update is already being installed")
        busy = true
        try {
            val file = File(context.cacheDir, "update.apk")
            val sha = MessageDigest.getInstance("SHA-256")
            get(DOWNLOADS + latest.file).inputStream.use { input ->
                file.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        sha.update(buf, 0, n)
                        out.write(buf, 0, n)
                    }
                }
            }
            val got = sha.digest().joinToString("") { "%02x".format(it) }
            if (got != latest.sha256) {
                file.delete()
                throw IOException("The download was damaged. Try again.")
            }

            val installer = context.packageManager.packageInstaller
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
            params.setAppPackageName(context.packageName)
            if (Build.VERSION.SDK_INT >= 31) {
                params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            }
            val id = installer.createSession(params)
            installer.openSession(id).use { session ->
                session.openWrite("EzRDP.apk", 0, file.length()).use { out ->
                    file.inputStream().use { it.copyTo(out) }
                    session.fsync(out)
                }
                val done = PendingIntent.getBroadcast(
                    context, 0, Intent(context, Result::class.java),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
                )
                session.commit(done.intentSender)
            }
            file.delete()
        } catch (e: Exception) {
            busy = false
            throw e
        }
    }

    /**
     * Called when the app opens: if automatic updates are on and it's been a while, checks, and
     * installs a newer build without asking (only if Android allows it without a prompt).
     */
    fun autoCheck(context: Context) {
        val s = Settings(context)
        val now = System.currentTimeMillis()
        if (!s.autoUpdate || now - s.lastUpdateCheck < CHECK_EVERY_MS || busy) return
        if (!allowed(context) || Build.VERSION.SDK_INT < 31 || ShareService.running) return
        s.lastUpdateCheck = now
        val app = context.applicationContext
        Thread {
            try {
                val latest = check(app) ?: return@Thread
                install(app, latest)
            } catch (_: Exception) {
                // Try again next time.
            }
        }.start()
    }

    /** Android's answer to an install. */
    class Result : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
                PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                    // This phone wants the person to confirm: show Android's "Update this app?".
                    @Suppress("DEPRECATION")
                    val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT) ?: return
                    context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                }
                PackageInstaller.STATUS_SUCCESS -> busy = false
                else -> {
                    busy = false
                    val why = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "unknown reason"
                    Handler(Looper.getMainLooper()).post {
                        Toast.makeText(context, "Update didn't install: $why", Toast.LENGTH_LONG).show()
                    }
                }
            }
        }
    }

    /** After an update: opens EzRDP again, or if Android doesn't allow that, offers a tap to open it. */
    class Restarted : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
            val open = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            try {
                context.startActivity(open)
            } catch (_: Exception) {}
            val nm = context.getSystemService(NotificationManager::class.java)
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "Updates", NotificationManager.IMPORTANCE_DEFAULT))
            val tap = PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_IMMUTABLE)
            val n = Notification.Builder(context, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle("EzRDP updated")
                .setContentText("Build ${currentBuild(context)}. Tap to open.")
                .setContentIntent(tap)
                .setAutoCancel(true)
                .setTimeoutAfter(10 * 60_000L)
                .build()
            try {
                nm.notify(7, n)
            } catch (_: SecurityException) {}
        }
    }
}
