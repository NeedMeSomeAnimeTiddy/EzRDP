package com.ezrdp.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import android.view.Surface
import android.view.WindowManager
import android.widget.Toast
import java.util.concurrent.atomic.AtomicInteger

/**
 * Shares this phone's screen with a PC, which can then control it (through [ControlService]).
 * Runs as a foreground service with a notification that has a Stop button.
 */
class ShareService : Service(), ConnectionListener {

    companion object {
        private const val TAG = "EzRDP"
        private const val CHANNEL = "share"
        private const val STOP = "com.ezrdp.app.STOP_SHARING"
        /** Frames sent but not yet shown on the PC. More than this = the link is slow; skip. */
        private const val MAX_IN_FLIGHT = 4

        @Volatile var running = false
            private set

        fun start(ctx: Context, pcId: String, liveHost: String?, livePort: Int, resultCode: Int, data: Intent) {
            val i = Intent(ctx, ShareService::class.java)
                .putExtra("pc", pcId)
                .putExtra("liveHost", liveHost)
                .putExtra("livePort", livePort)
                .putExtra("code", resultCode)
                .putExtra("data", data)
            ctx.startForegroundService(i)
        }
    }

    private val ui = Handler(Looper.getMainLooper())
    @Volatile private var conn: Connection? = null
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    @Volatile private var codec: MediaCodec? = null
    private var drainThread: Thread? = null
    private var pcName = "PC"
    private var width = 0
    private var height = 0

    private val frameId = AtomicInteger(0)
    @Volatile private var lastAck = 0
    @Volatile private var skipping = false
    private var config: ByteArray? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (intent == null || running) return START_NOT_STICKY
        val pc = PcStore(this).get(intent.getStringExtra("pc") ?: "")
        @Suppress("DEPRECATION")
        val data = intent.getParcelableExtra<Intent>("data")
        if (pc == null || data == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        pcName = pc.name
        // Must be in the foreground before touching the screen recording.
        goForeground("Connecting to ${pc.name}…")
        running = true

        val mpm = getSystemService(MediaProjectionManager::class.java)
        val p = try {
            mpm.getMediaProjection(intent.getIntExtra("code", 0), data)
        } catch (e: Exception) {
            Log.e(TAG, "no screen recording permission", e)
            null
        }
        if (p == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        projection = p
        p.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                ui.post { stopSelf() }
            }
        }, ui)

        val liveHost = intent.getStringExtra("liveHost")
        val port = intent.getIntExtra("livePort", pc.port)
        Thread {
            var error: Exception? = null
            for ((addr, timeout) in pc.addresses(liveHost)) {
                if (!running) return@Thread
                val c = Connection(addr, port, pc.id.fromHex(), pc.deviceId, pc.key, LoginOptions(role = Msg.ROLE_SHARE_PHONE), this, timeout)
                try {
                    c.open()
                    conn = c
                    ui.post { startVideo() }
                    return@Thread
                } catch (e: Exception) {
                    c.close()
                    error = e
                    if (e is NotTrustedException || e is VersionException || e is PausedException) break
                }
            }
            ui.post {
                val why = when (error) {
                    is PausedException -> "Connections are paused on ${pc.name}."
                    is VersionException -> (error as VersionException).advice(pc.name)
                    is NotTrustedException -> "${pc.name} no longer trusts this phone."
                    else -> "Can't reach ${pc.name}."
                }
                Toast.makeText(this, why, Toast.LENGTH_LONG).show()
                stopSelf()
            }
        }.start()
        return START_NOT_STICKY
    }

    private fun goForeground(text: String) {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Sharing with PC", NotificationManager.IMPORTANCE_LOW))
        val stop = PendingIntent.getService(
            this, 0, Intent(this, ShareService::class.java).setAction(STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .setContentTitle("EzRDP")
            .setContentText(text)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        } else {
            startForeground(1, n)
        }
    }

    // ---------- Screen video ----------

    /** Screen size shrunk so the long side is at most 1280 (plenty for a PC window, quick to send). */
    private fun videoSize(): Pair<Int, Int> {
        val wm = getSystemService(WindowManager::class.java)
        val (w, h) = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            wm.currentWindowMetrics.bounds.let { it.width() to it.height() }
        } else {
            val m = android.util.DisplayMetrics()
            @Suppress("DEPRECATION")
            wm.defaultDisplay.getRealMetrics(m)
            m.widthPixels to m.heightPixels
        }
        val s = minOf(1f, 1280f / maxOf(w, h))
        // Multiples of 16 suit every encoder.
        return (((w * s).toInt() + 8) / 16 * 16).coerceAtLeast(16) to (((h * s).toInt() + 8) / 16 * 16).coerceAtLeast(16)
    }

    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(id: Int) {}
        override fun onDisplayRemoved(id: Int) {}
        override fun onDisplayChanged(id: Int) {
            // Phone rotated: restart the encoder at the new shape.
            val (w, h) = videoSize()
            if (w != width || h != height) startVideo()
        }
    }

    private fun startVideo() {
        val c = conn ?: return
        val p = projection ?: return
        val (w, h) = videoSize()
        stopEncoder()
        width = w
        height = h
        val enc = try {
            createEncoder(w, h)
        } catch (e: Exception) {
            Log.e(TAG, "screen encoder failed", e)
            Toast.makeText(this, "This phone couldn't record its screen.", Toast.LENGTH_LONG).show()
            stopSelf()
            return
        }
        val surface = enc.createInputSurface()
        enc.start()
        codec = enc
        config = null
        skipping = false
        c.sendScreenConfig(w, h)
        val dpi = resources.displayMetrics.densityDpi
        val vd = display
        if (vd == null) {
            display = p.createVirtualDisplay("EzRDP", w, h, dpi, DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, null)
            getSystemService(DisplayManager::class.java).registerDisplayListener(displayListener, ui)
        } else {
            // Android 14+ allows only one virtual display per permission, so reuse it.
            vd.resize(w, h, dpi)
            vd.surface = surface
        }
        drainThread = Thread({ drain(enc, c) }, "ezrdp-share-out").also { it.start() }
        goForeground("$pcName can see and control this phone")
    }

    private fun createEncoder(w: Int, h: Int): MediaCodec {
        val f = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h)
        f.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
        f.setInteger(MediaFormat.KEY_BIT_RATE, 5_000_000)
        f.setInteger(MediaFormat.KEY_FRAME_RATE, 60)
        f.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10)
        // When the screen is still, resend the last picture now and then so it sharpens up.
        f.setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 250_000)
        f.setInteger(MediaFormat.KEY_PRIORITY, 0)
        f.setInteger(MediaFormat.KEY_LATENCY, 1)
        f.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
        f.setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.AVCLevel41)
        val enc = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        try {
            enc.configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (e: Exception) {
            // Some encoders reject the extra settings; try the basics.
            enc.reset()
            for (k in listOf(MediaFormat.KEY_LATENCY, MediaFormat.KEY_PRIORITY, MediaFormat.KEY_PROFILE, MediaFormat.KEY_LEVEL)) {
                f.removeKey(k)
            }
            enc.configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        }
        return enc
    }

    private fun drain(enc: MediaCodec, c: Connection) {
        val info = MediaCodec.BufferInfo()
        var buf = ByteArray(256 * 1024)
        try {
            while (codec === enc) {
                val index = enc.dequeueOutputBuffer(info, 50_000)
                if (index < 0) continue
                val out = enc.getOutputBuffer(index)
                if (out != null && info.size > 0) {
                    if (buf.size < info.size) buf = ByteArray(info.size * 2)
                    out.position(info.offset)
                    out.get(buf, 0, info.size)
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        config = buf.copyOf(info.size)
                    } else {
                        send(c, enc, buf, info.size, info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0)
                    }
                }
                enc.releaseOutputBuffer(index, false)
            }
        } catch (_: Exception) {
            // Encoder stopped.
        }
    }

    private fun send(c: Connection, enc: MediaCodec, data: ByteArray, size: Int, key: Boolean) {
        val inFlight = frameId.get() - lastAck
        if (!key && (skipping || inFlight >= MAX_IN_FLIGHT || c.queued() > 8)) {
            // The PC is behind: drop frames until a fresh full picture, so delay doesn't build up.
            if (!skipping) {
                skipping = true
                requestKeyframe(enc)
            }
            return
        }
        skipping = false
        val id = frameId.incrementAndGet()
        val cfg = config
        if (key && cfg != null) {
            val all = ByteArray(cfg.size + size)
            System.arraycopy(cfg, 0, all, 0, cfg.size)
            System.arraycopy(data, 0, all, cfg.size, size)
            c.sendScreenFrame(id, true, all, 0, all.size)
        } else {
            c.sendScreenFrame(id, key, data, 0, size)
        }
    }

    private fun requestKeyframe(enc: MediaCodec) {
        try {
            enc.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        } catch (_: Exception) {}
    }

    private fun stopEncoder() {
        val enc = codec ?: return
        codec = null
        drainThread?.join(300)
        drainThread = null
        try { enc.stop() } catch (_: Exception) {}
        try { enc.release() } catch (_: Exception) {}
    }

    // ---------- From the PC ----------

    override fun onTouch(action: Int, x: Float, y: Float) {
        ui.post { ControlService.instance?.touch(action, x, y) }
    }

    override fun onNav(what: Int) {
        ui.post { ControlService.instance?.nav(what) }
    }

    override fun onType(text: String) {
        ui.post { ControlService.instance?.type(text) }
    }

    override fun onSwipe(x: Float, y: Float, dx: Float, dy: Float) {
        ui.post { ControlService.instance?.swipe(x, y, dx, dy) }
    }

    override fun onFrameAck(id: Int) {
        if (id - lastAck > 0) lastAck = id
    }

    override fun onKeyframeRequest() {
        codec?.let { requestKeyframe(it) }
    }

    override fun onDisconnected(error: Exception?) {
        ui.post { stopSelf() }
    }

    override fun onDestroy() {
        running = false
        try {
            getSystemService(DisplayManager::class.java).unregisterDisplayListener(displayListener)
        } catch (_: Exception) {}
        conn?.close()
        conn = null
        stopEncoder()
        display?.release()
        display = null
        projection?.stop()
        projection = null
        super.onDestroy()
    }
}
