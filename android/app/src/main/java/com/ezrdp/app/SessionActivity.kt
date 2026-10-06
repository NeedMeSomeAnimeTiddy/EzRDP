package com.ezrdp.app

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Color
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import android.os.Build

class SessionActivity : Activity(), ConnectionListener {
    private val ui = Handler(Looper.getMainLooper())
    @Volatile private var conn: Connection? = null
    private lateinit var remote: RemoteView
    private lateinit var keyInput: KeyInput
    private lateinit var status: TextView
    private lateinit var toolbar: View
    private lateinit var keysBar: View
    private lateinit var soundBtn: TextView
    private lateinit var modeBtn: TextView
    private val modifierButtons = HashMap<Int, TextView>()
    private val heldModifiers = HashSet<Int>()

    private val decoder = VideoDecoder { conn?.requestKeyframe() }
    private val audio = AudioPlayer()

    private lateinit var host: String
    private var port = 7270
    private lateinit var password: String
    private var quality = 1
    private var soundOn = false
    private var monitors: List<MonitorInfo> = emptyList()
    private var currentMonitor = 0
    private var chosenMonitor: Int? = null
    private var running = false
    private var attempt = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        host = intent.getStringExtra("host") ?: ""
        port = intent.getIntExtra("port", 7270)
        password = intent.getStringExtra("password") ?: ""
        quality = intent.getIntExtra("quality", 1)
        val prefs = getSharedPreferences("ezrdp", MODE_PRIVATE)
        soundOn = prefs.getBoolean("sound", false)

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }

        val root = FrameLayout(this)
        root.setBackgroundColor(Color.BLACK)

        remote = RemoteView(this) { conn }
        remote.trackpad = prefs.getBoolean("trackpad", true)
        remote.onSurface = { decoder.setSurface(it) }
        remote.onAction = { releaseModifiers() }
        root.addView(remote, FrameLayout.LayoutParams(-1, -1))

        keyInput = KeyInput(this, { conn }) { releaseModifiers() }
        root.addView(keyInput, FrameLayout.LayoutParams(1, 1))

        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 16f
            gravity = Gravity.CENTER
            setPadding(dp(20), dp(12), dp(20), dp(12))
            background = rounded(0xDD202228.toInt(), dp(12).toFloat())
        }
        root.addView(status, FrameLayout.LayoutParams(-2, -2, Gravity.CENTER))

        val bars = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
        }
        toolbar = buildToolbar()
        keysBar = buildKeysBar()
        toolbar.visibility = View.GONE
        keysBar.visibility = View.GONE
        val handle = pill("☰") { toggle(toolbar) }.apply { alpha = 0.55f }
        bars.addView(handle)
        bars.addView(toolbar)
        bars.addView(keysBar)
        root.addView(bars, FrameLayout.LayoutParams(-1, -2, Gravity.TOP).apply { topMargin = dp(4) })

        setContentView(root)
        hideSystemBars()
    }

    private fun spaced(v: View) = LinearLayout.LayoutParams(-2, -2).apply { setMargins(dp(3), dp(4), dp(3), 0) }

    private fun scrollRow(vararg items: View): View {
        val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        for (v in items) row.addView(v, spaced(v))
        return HorizontalScrollView(this).apply {
            isHorizontalScrollBarEnabled = false
            addView(row)
        }
    }

    private fun buildToolbar(): View {
        soundBtn = pill("Sound") { setSound(!soundOn) }.also { it.setActive(soundOn) }
        modeBtn = pill(if (remote.trackpad) "Trackpad" else "Touch") { toggleMode() }
        return scrollRow(
            pill("Keyboard") { keyInput.show() },
            pill("Keys") { toggle(keysBar) },
            pill("Screen") { pickMonitor() },
            soundBtn,
            modeBtn,
            pill("Zoom 1x") { remote.resetZoom() },
            pill("Quality") { pickQuality() },
            pill("Exit") { finish() },
        )
    }

    private fun buildKeysBar(): View {
        fun key(label: String, vk: Int) = pill(label) {
            conn?.tapKey(vk)
            releaseModifiers()
        }
        fun mod(label: String, vk: Int) = pill(label) { toggleModifier(vk) }.also { modifierButtons[vk] = it }
        val fkeys = (1..12).map { n -> key("F$n", VK.F1 + n - 1) }
        return scrollRow(
            key("Esc", VK.ESC),
            key("Tab", VK.TAB),
            mod("Ctrl", VK.CTRL),
            mod("Alt", VK.ALT),
            mod("Shift", VK.SHIFT),
            mod("Win", VK.WIN),
            key("←", VK.LEFT),
            key("↑", VK.UP),
            key("↓", VK.DOWN),
            key("→", VK.RIGHT),
            key("Del", VK.DEL),
            key("⌫", VK.BACK),
            key("Enter", VK.ENTER),
            key("Home", VK.HOME),
            key("End", VK.END),
            key("PgUp", VK.PGUP),
            key("PgDn", VK.PGDN),
            key("PrtSc", VK.PRTSC),
            pill("Task Mgr") { combo(VK.CTRL, VK.SHIFT, VK.ESC) },
            pill("Alt+Tab") { combo(VK.ALT, VK.TAB) },
            pill("Desktop") { combo(VK.WIN, 'D'.code) },
            *fkeys.toTypedArray(),
        )
    }

    private fun combo(vararg keys: Int) {
        val c = conn ?: return
        for (k in keys) c.key(k, true)
        for (k in keys.reversed()) c.key(k, false)
    }

    /** Modifiers stay held until the next key, click or typed text, then let go. */
    private fun toggleModifier(vk: Int) {
        val on = !heldModifiers.contains(vk)
        if (on) heldModifiers.add(vk) else heldModifiers.remove(vk)
        conn?.key(vk, on)
        modifierButtons[vk]?.setActive(on)
    }

    private fun releaseModifiers() {
        if (heldModifiers.isEmpty()) return
        for (vk in heldModifiers) {
            conn?.key(vk, false)
            modifierButtons[vk]?.setActive(false)
        }
        heldModifiers.clear()
    }

    private fun toggle(v: View) {
        v.visibility = if (v.visibility == View.VISIBLE) View.GONE else View.VISIBLE
        if (v == toolbar && v.visibility == View.GONE) keysBar.visibility = View.GONE
    }

    private fun toggleMode() {
        remote.trackpad = !remote.trackpad
        modeBtn.text = if (remote.trackpad) "Trackpad" else "Touch"
        getSharedPreferences("ezrdp", MODE_PRIVATE).edit().putBoolean("trackpad", remote.trackpad).apply()
    }

    private fun setSound(on: Boolean) {
        soundOn = on
        soundBtn.setActive(on)
        conn?.setAudio(on)
        if (!on) audio.release()
        getSharedPreferences("ezrdp", MODE_PRIVATE).edit().putBoolean("sound", on).apply()
    }

    private fun pickMonitor() {
        if (monitors.isEmpty()) return
        val labels = monitors.map {
            val main = if (it.primary) "  (main)" else ""
            val cur = if (it.index == currentMonitor) "✓ " else ""
            "$cur${it.name} — ${it.width}×${it.height}$main"
        }.toTypedArray()
        AlertDialog.Builder(this)
            .setTitle("Show which screen?")
            .setItems(labels) { _, i ->
                chosenMonitor = monitors[i].index
                conn?.selectMonitor(monitors[i].index)
            }
            .show()
    }

    private fun pickQuality() {
        val labels = arrayOf("High (sharpest, needs fast internet)", "Balanced", "Data saver (mobile data)")
        AlertDialog.Builder(this)
            .setTitle("Quality")
            .setSingleChoiceItems(labels, quality) { d, i ->
                quality = i
                conn?.setQuality(i)
                getSharedPreferences("ezrdp", MODE_PRIVATE).edit().putInt("quality", i).apply()
                d.dismiss()
            }
            .show()
    }

    @Suppress("DEPRECATION")
    private fun hideSystemBars() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
            window.insetsController?.let {
                it.hide(WindowInsets.Type.systemBars())
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            window.decorView.systemUiVisibility = (View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                or View.SYSTEM_UI_FLAG_FULLSCREEN
                or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN)
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideSystemBars()
    }

    // ---------- Connection lifecycle ----------

    override fun onStart() {
        super.onStart()
        running = true
        attempt = 0
        connect()
    }

    override fun onStop() {
        super.onStop()
        running = false
        ui.removeCallbacksAndMessages(null)
        conn?.close()
        conn = null
        audio.release()
    }

    override fun onDestroy() {
        super.onDestroy()
        decoder.release()
    }

    private fun showStatus(text: String?) {
        status.text = text ?: ""
        status.visibility = if (text == null) View.GONE else View.VISIBLE
    }

    private fun connect() {
        showStatus(if (attempt == 0) "Connecting to $host…" else "Reconnecting…")
        Thread {
            val c = Connection(host, port, password, quality, this)
            try {
                c.open()
                ui.post {
                    if (!running) {
                        c.close()
                        return@post
                    }
                    conn = c
                    attempt = 0
                    showStatus("Waiting for picture…")
                    if (soundOn) c.setAudio(true)
                    chosenMonitor?.let { c.selectMonitor(it) }
                }
            } catch (e: AuthException) {
                c.close()
                ui.post { fail("Wrong password. Check the password shown on your PC.") }
            } catch (e: Exception) {
                c.close()
                ui.post { retry() }
            }
        }.start()
    }

    private fun retry() {
        if (!running) return
        attempt++
        if (attempt > 20) {
            fail("Can't reach $host:$port.\n\nIs EzRDP running on the PC, and is the address right?")
            return
        }
        showStatus("Can't reach the PC, retrying…")
        ui.postDelayed({ if (running) connect() }, 1500)
    }

    private fun fail(msg: String) {
        if (isFinishing) return
        AlertDialog.Builder(this)
            .setTitle("Couldn't connect")
            .setMessage(msg)
            .setPositiveButton("OK") { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
    }

    // ---------- ConnectionListener (network thread) ----------

    override fun onMonitors(list: List<MonitorInfo>) {
        ui.post { monitors = list }
    }

    override fun onVideoConfig(monitor: Int, width: Int, height: Int) {
        decoder.configure(width, height)
        ui.post {
            currentMonitor = monitor
            remote.setVideoSize(width, height)
        }
    }

    private var gotFrame = false

    override fun onVideoFrame(data: ByteArray, offset: Int, length: Int, keyframe: Boolean) {
        decoder.decode(data, offset, length, keyframe)
        if (!gotFrame) {
            gotFrame = true
            ui.post { showStatus(null) }
        }
    }

    override fun onCursor(x: Float, y: Float, visible: Boolean) {
        ui.post { remote.setRemoteCursor(x, y, visible) }
    }

    override fun onAudioConfig(sampleRate: Int, channels: Int) {
        if (soundOn) audio.configure(sampleRate, channels)
    }

    override fun onAudio(data: ByteArray, offset: Int, length: Int) {
        if (soundOn) audio.write(data, offset, length)
    }

    override fun onDisconnected(error: Exception?) {
        if (error == null) return // we hung up on purpose
        ui.post {
            if (conn == null) return@post
            conn = null
            gotFrame = false
            heldModifiers.clear()
            modifierButtons.values.forEach { it.setActive(false) }
            if (running) retry()
        }
    }
}
