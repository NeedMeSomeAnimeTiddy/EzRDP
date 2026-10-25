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

class SessionActivity : Activity() {
    companion object {
        /** Biggest picture this phone can decode; looked up once. */
        val decoderLimit: Pair<Int, Int> by lazy { VideoDecoder.maxSize() }
    }

    private val ui = Handler(Looper.getMainLooper())
    @Volatile private var conn: Connection? = null
    /** The connection being opened right now (not yet [conn]). */
    @Volatile private var opening: Connection? = null
    /** Bumped on every connect and on leaving: anything from an older attempt is ignored. */
    private var generation = 0
    /** The current connection's first picture has shown. */
    private var pictureShown = false
    private var notice: AlertDialog? = null
    private lateinit var remote: RemoteView
    private lateinit var keyInput: KeyInput
    private lateinit var status: TextView
    private lateinit var statusButton: TextView
    private lateinit var settings: Settings
    private lateinit var controls: Controls
    private lateinit var doneEditing: TextView
    private lateinit var statusBox: View
    private lateinit var toolbar: View
    private lateinit var keysBar: View
    private lateinit var soundBtn: TextView
    private lateinit var modeBtn: TextView
    private val modifierButtons = HashMap<Int, TextView>()
    private val heldModifiers = HashSet<Int>()

    private val decoder = VideoDecoder { (conn ?: opening)?.requestKeyframe() }.also { d ->
        d.onPicture = {
            ui.post {
                pictureShown = true
                if (conn != null) showStatus(null)
            }
        }
        d.onStuck = { ui.post { pictureStuck() } }
    }
    private val audio = AudioPlayer()

    /** Until the decoder has a full picture, keep asking the PC for one. A single request can get
     *  lost, and the PC sends nothing new while its screen is still, so the phone would stay on
     *  "Waiting for picture" (or a frozen picture) until something on the PC changed. */
    private val keyframeTicker = object : Runnable {
        override fun run() {
            val c = conn ?: return
            if (decoder.needsKeyframe) c.requestKeyframe()
            ui.postDelayed(this, 1000)
        }
    }

    private lateinit var store: PcStore
    private lateinit var pc: SavedPc
    private var liveHost: String? = null
    private var port = 7270
    private var quality = 1
    private var gpuEncoder = true
    private var soundOn = false
    private var monitors: List<MonitorInfo> = emptyList()
    private var currentMonitor = 0
    private var chosenMonitor: Int? = null
    private var running = false
    private var attempt = 0
    /** True once this screen has been connected; after that we keep retrying for longer. */
    private var everConnected = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        store = PcStore(this)
        val saved = store.get(intent.getStringExtra("pc") ?: "")
        if (saved == null) {
            finish()
            return
        }
        pc = saved
        liveHost = intent.getStringExtra("liveHost")
        port = intent.getIntExtra("livePort", pc.port)
        settings = Settings(this)
        applyOrientation()

        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }

        val root = FrameLayout(this)
        root.setBackgroundColor(Color.BLACK)

        remote = RemoteView(this) { conn }
        remote.onSurface = { decoder.setSurface(it) }
        remote.onAction = { releaseModifiers() }
        root.addView(remote, FrameLayout.LayoutParams(-1, -1))

        keyInput = KeyInput(this, { conn }) { releaseModifiers() }
        root.addView(keyInput, FrameLayout.LayoutParams(1, 1))

        controls = Controls(this, root, remote, { conn }, { keyInput.show() }, { nextMonitor() }) { releaseModifiers() }
        root.addOnLayoutChangeListener { _, l, t, r, b, ol, ot, or, ob ->
            if (r - l != or - ol || b - t != ob - ot) root.post { controls.relayout() }
        }

        // Messages (connecting, errors) in a box that always fits: it never gets wider than the
        // screen, and long text scrolls inside it.
        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 16f
            gravity = Gravity.CENTER
            setLineSpacing(0f, 1.15f)
        }
        statusButton = pill("Cancel") { finish() }
        statusBox = object : LinearLayout(this) {
            override fun onMeasure(w: Int, h: Int) {
                val parentW = View.MeasureSpec.getSize(w)
                val parentH = View.MeasureSpec.getSize(h)
                val maxW = minOf(dp(380), parentW - dp(32))
                val maxH = parentH - dp(32)
                super.onMeasure(
                    View.MeasureSpec.makeMeasureSpec(maxW.coerceAtLeast(dp(120)), View.MeasureSpec.AT_MOST),
                    View.MeasureSpec.makeMeasureSpec(maxH.coerceAtLeast(dp(120)), View.MeasureSpec.AT_MOST),
                )
            }
        }.apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setPadding(dp(22), dp(16), dp(22), dp(14))
            background = rounded(0xEE202228.toInt(), dp(14).toFloat())
            // Leaves room for the button below, however long the message is.
            addView(object : android.widget.ScrollView(context) {
                override fun onMeasure(w: Int, h: Int) {
                    val room = (root.height - dp(130)).coerceAtLeast(dp(80))
                    super.onMeasure(w, View.MeasureSpec.makeMeasureSpec(room, View.MeasureSpec.AT_MOST))
                }
            }.apply {
                isVerticalScrollBarEnabled = false
                addView(status)
            }, LinearLayout.LayoutParams(-2, -2))
            addView(statusButton, LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(12) })
        }
        root.addView(statusBox, FrameLayout.LayoutParams(-2, -2, Gravity.CENTER))

        val bars = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
        }
        toolbar = buildToolbar()
        keysBar = buildKeysBar()
        toolbar.visibility = View.GONE
        keysBar.visibility = View.GONE
        val handle = pill("☰") { toggle(toolbar) }.apply { alpha = 0.7f }
        bars.addView(handle, LinearLayout.LayoutParams(-2, -2))
        bars.addView(toolbar)
        bars.addView(keysBar)
        root.addView(bars, FrameLayout.LayoutParams(-1, -2, Gravity.TOP).apply { topMargin = dp(4) })

        doneEditing = pill("Done moving buttons") {
            controls.setEditing(false)
            doneEditing.visibility = View.GONE
        }.apply {
            visibility = View.GONE
            setActive(true)
        }
        root.addView(doneEditing, FrameLayout.LayoutParams(-2, -2, Gravity.CENTER))

        // When the phone keyboard opens, shrink the PC picture to the space above it (instead of
        // the keyboard covering half the screen), and give the full screen back when it closes.
        root.setOnApplyWindowInsetsListener { _, insets ->
            val ime = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) insets.getInsets(WindowInsets.Type.ime()).bottom else 0
            val lp = remote.layoutParams as FrameLayout.LayoutParams
            if (lp.bottomMargin != ime) {
                lp.bottomMargin = ime
                remote.layoutParams = lp
                root.post { refreshLayout() }
            }
            insets
        }
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
        modeBtn = pill(modeLabel()) { toggleMode() }
        return scrollRow(
            pill("Keyboard") { keyInput.show() },
            pill("Keys") { toggle(keysBar) },
            pill("Screen") { pickMonitor() },
            soundBtn,
            modeBtn,
            pill("Quality") { pickQuality() },
            pill("Edit layout") { editLayout() },
            pill("Settings") { startActivity(android.content.Intent(this, SettingsActivity::class.java)) },
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
            pill("Copy") { combo(VK.CTRL, 'C'.code) },
            pill("Paste") { combo(VK.CTRL, 'V'.code) },
            pill("Cut") { combo(VK.CTRL, 'X'.code) },
            pill("Undo") { combo(VK.CTRL, 'Z'.code) },
            pill("Redo") { combo(VK.CTRL, 'Y'.code) },
            pill("Select all") { combo(VK.CTRL, 'A'.code) },
            pill("Task Mgr") { combo(VK.CTRL, VK.SHIFT, VK.ESC) },
            pill("Alt+Tab") { combo(VK.ALT, VK.TAB) },
            pill("Desktop") { combo(VK.WIN, 'D'.code) },
            *fkeys.toTypedArray(),
        )
    }

    private fun editLayout() {
        if (!settings.showControls) {
            settings.showControls = true
            controls.build()
        }
        toolbar.visibility = View.GONE
        keysBar.visibility = View.GONE
        controls.setEditing(true)
        doneEditing.visibility = View.VISIBLE
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

    private fun modeLabel() = "Mode: " + Settings.MODE_NAMES[remote.mode]

    /** Cycles Both → Trackpad → Touch. */
    private fun toggleMode() {
        remote.mode = (remote.mode + 1) % 3
        settings.mouseMode = remote.mode
        modeBtn.text = modeLabel()
        android.widget.Toast.makeText(this, when (remote.mode) {
            Settings.MODE_TRACKPAD -> "Trackpad: drag moves the pointer, tap clicks where it is"
            Settings.MODE_TOUCH -> "Touch: the pointer follows your finger"
            else -> "Both: tap to click there, drag to move the pointer"
        }, android.widget.Toast.LENGTH_SHORT).show()
    }

    private fun setSound(on: Boolean) {
        soundOn = on
        soundBtn.setActive(on)
        conn?.setAudio(on)
        if (!on) audio.release()
        settings.sound = on
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
                settings.quality = i
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

    /** Re-measures and redraws everything (after the keyboard, rotation or coming back to the app). */
    private fun refreshLayout() {
        val root = remote.parent as? View ?: return
        root.requestLayout()
        root.invalidate()
        remote.invalidate()
        controls.relayout()
    }

    private fun applyOrientation() {
        requestedOrientation = if (settings.landscape) {
            android.content.pm.ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE
        } else {
            android.content.pm.ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        }
    }

    /** The on-screen "Screen" button: show the PC's next monitor. */
    private fun nextMonitor() {
        if (monitors.size < 2) {
            android.widget.Toast.makeText(this, "This PC has only one screen", android.widget.Toast.LENGTH_SHORT).show()
            return
        }
        val i = monitors.indexOfFirst { it.index == currentMonitor }
        val next = monitors[(i + 1) % monitors.size]
        chosenMonitor = next.index
        conn?.selectMonitor(next.index)
        android.widget.Toast.makeText(this, "Showing ${next.name}${if (next.primary) " (main)" else ""}", android.widget.Toast.LENGTH_SHORT).show()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            hideSystemBars()
            remote.post { refreshLayout() }
        }
    }

    // ---------- Connection lifecycle ----------

    override fun onStart() {
        super.onStart()
        if (!::pc.isInitialized) return
        quality = settings.quality
        soundOn = settings.sound
        gpuEncoder = settings.gpuEncoder
        running = true
        attempt = 0
        connect()
    }

    /** Settings opens on top without disconnecting; pick up its changes when it closes. */
    override fun onResume() {
        super.onResume()
        if (!::pc.isInitialized) return
        applyOrientation()
        remote.apply(settings)
        modeBtn.text = modeLabel()
        controls.build()
        if (settings.quality != quality) {
            quality = settings.quality
            conn?.setQuality(quality)
        }
        if (settings.sound != soundOn) setSound(settings.sound)
        if (settings.gpuEncoder != gpuEncoder) {
            // Only takes effect on a new connection.
            gpuEncoder = settings.gpuEncoder
            conn?.close()
            conn = null
            if (running) connect()
        }
    }

    override fun onStop() {
        super.onStop()
        running = false
        generation++
        ui.removeCallbacksAndMessages(null)
        opening?.close()
        opening = null
        conn?.close()
        conn = null
        forgetModifiers()
        audio.release()
    }

    /** The PC lets go of held keys when a connection ends; make the buttons match. */
    private fun forgetModifiers() {
        heldModifiers.clear()
        modifierButtons.values.forEach { it.setActive(false) }
    }

    override fun onDestroy() {
        super.onDestroy()
        decoder.release()
    }

    private fun showStatus(text: String?, button: String = "Cancel") {
        status.text = text ?: ""
        // Short messages centred; long ones (with bullet points) read better from the left.
        status.gravity = if ((text ?: "").contains('•')) Gravity.START else Gravity.CENTER
        statusButton.text = button
        statusBox.visibility = if (text == null) View.GONE else View.VISIBLE
        statusBox.requestLayout()
    }

    /** The decoder takes video but shows nothing: usually the picture is too big for this phone. */
    private fun pictureStuck() {
        val c = conn ?: return
        if (quality < 2) {
            quality++
            c.setQuality(quality)
            decoder.release()
            android.widget.Toast.makeText(
                this,
                "The picture didn't show at that quality on this phone, so it was lowered.",
                android.widget.Toast.LENGTH_LONG,
            ).show()
        } else if (gpuEncoder) {
            android.widget.Toast.makeText(
                this,
                "Picture problem. Try turning off \"Use the PC's graphics card\" in Settings.",
                android.widget.Toast.LENGTH_LONG,
            ).show()
        }
    }

    private fun connect() {
        showStatus(if (attempt == 0) "Connecting to ${pc.name}…" else "Reconnecting…")
        // Only ever one attempt at a time: a newer one makes older ones give up.
        val gen = ++generation
        opening?.close()
        opening = null
        // Fresh decoder before the connection starts sending, so its first picture isn't lost.
        decoder.release()
        pictureShown = false
        Thread {
            var lastError: Exception? = null
            val (maxW, maxH) = decoderLimit
            val options = LoginOptions(quality, Msg.ROLE_CONTROL_PC, maxW, maxH, gpuEncoder)
            for ((addr, timeout) in pc.addresses(liveHost)) {
                if (!running || gen != generation) return@Thread
                val events = Events()
                val c = Connection(addr, port, pc.id.fromHex(), pc.deviceId, pc.key, options, events, timeout)
                events.c = c
                opening = c
                try {
                    c.open()
                    ui.post {
                        if (opening === c) opening = null
                        if (!running || gen != generation || conn != null) {
                            c.close()
                            return@post
                        }
                        if (c.closed) {
                            // It dropped before we could start using it.
                            retry()
                            return@post
                        }
                        conn = c
                        attempt = 0
                        everConnected = true
                        forgetModifiers()
                        showStatus(if (pictureShown) null else "Waiting for picture…")
                        if (soundOn) c.setAudio(true)
                        chosenMonitor?.let { c.selectMonitor(it) }
                        ui.removeCallbacks(keyframeTicker)
                        ui.postDelayed(keyframeTicker, 1000)
                    }
                    return@Thread
                } catch (e: Exception) {
                    c.close()
                    if (opening === c) opening = null
                    lastError = e
                    if (e is NotTrustedException || e is VersionException || e is PausedException) break
                }
            }
            ui.post {
                if (gen != generation) return@post
                when (lastError) {
                    is NotTrustedException -> {
                        store.remove(pc.id)
                        fail("${pc.name} no longer trusts this phone. Add it again from the list while you're on the same Wi-Fi.")
                    }
                    is PausedException -> fail("Connections are paused on ${pc.name}. Right-click the EzRDP icon on the PC and choose Resume.")
                    is VersionException -> fail((lastError as VersionException).advice(pc.name))
                    else -> retry()
                }
            }
        }.start()
    }

    private fun retry() {
        if (!running) return
        attempt++
        if (attempt > (if (everConnected) 20 else 2)) {
            val away = liveHost == null
            fail(
                "Can't reach ${pc.name}.\n\n" +
                    "• Is the PC on, with EzRDP running (icon next to the clock)?\n" +
                    (if (away) "• From outside your home, your router must let EzRDP in: check \"Connection info\" in the EzRDP tray menu on the PC.\n" else "") +
                    "• At home, make sure the phone is on the same Wi-Fi.",
            )
            return
        }
        showStatus("Can't reach ${pc.name}, retrying…")
        val gen = generation
        ui.postDelayed({ if (running && gen == generation) connect() }, 1500)
    }

    private fun fail(msg: String) {
        if (isFinishing) return
        running = false
        showStatus("Couldn't connect\n\n$msg", "OK")
    }

    // ---------- Connection events (network thread) ----------

    /** One connection's events. Ones from a connection that is no longer in use are ignored,
     *  so an old connection ending can't cut off the new one (picture showing, taps going nowhere). */
    private inner class Events : ConnectionListener {
        lateinit var c: Connection
        private fun live() = c === conn || c === opening

        override fun onMonitors(list: List<MonitorInfo>) { if (live()) this@SessionActivity.onMonitors(list) }
        override fun onVideoConfig(monitor: Int, width: Int, height: Int) {
            if (live()) this@SessionActivity.onVideoConfig(monitor, width, height)
        }
        override fun onVideoFrame(data: ByteArray, offset: Int, length: Int, keyframe: Boolean) {
            if (live()) this@SessionActivity.onVideoFrame(data, offset, length, keyframe)
        }
        override fun onCursor(x: Float, y: Float, visible: Boolean) { if (live()) this@SessionActivity.onCursor(x, y, visible) }
        override fun onAudioConfig(sampleRate: Int, channels: Int) {
            if (live()) this@SessionActivity.onAudioConfig(sampleRate, channels)
        }
        override fun onAudio(data: ByteArray, offset: Int, length: Int) {
            if (live()) this@SessionActivity.onAudio(data, offset, length)
        }
        override fun onAddresses(home: String, internet: String, port: Int) {
            if (live()) this@SessionActivity.onAddresses(home, internet, port)
        }
        override fun onNotice(text: String) { if (live()) this@SessionActivity.onNotice(text) }
        override fun onBye(reason: Int) { ui.post { if (c === conn) this@SessionActivity.onBye(reason) } }
        override fun onDisconnected(error: Exception?) {
            if (error == null) return // we hung up on purpose
            ui.post { if (c === conn) this@SessionActivity.onDisconnected() }
        }
    }

    private fun onMonitors(list: List<MonitorInfo>) {
        ui.post { monitors = list }
    }

    private fun onVideoConfig(monitor: Int, width: Int, height: Int) {
        decoder.configure(width, height)
        ui.post {
            val newScreen = monitor != currentMonitor
            currentMonitor = monitor
            remote.setVideoSize(width, height, newScreen)
        }
    }

    private fun onVideoFrame(data: ByteArray, offset: Int, length: Int, keyframe: Boolean) {
        decoder.decode(data, offset, length, keyframe)
    }

    private fun onCursor(x: Float, y: Float, visible: Boolean) {
        ui.post { remote.setRemoteCursor(x, y, visible) }
    }

    private fun onAudioConfig(sampleRate: Int, channels: Int) {
        if (soundOn) audio.configure(sampleRate, channels)
    }

    private fun onAudio(data: ByteArray, offset: Int, length: Int) {
        if (soundOn) audio.write(data, offset, length)
    }

    private fun onAddresses(home: String, internet: String, port: Int) {
        ui.post {
            if (home.isNotEmpty()) pc.home = home
            if (internet.isNotEmpty()) pc.internet = internet
            pc.port = port
            store.save(pc)
        }
    }

    private fun onBye(reason: Int) {
        running = false
        conn?.close()
        conn = null
        forgetModifiers()
        controls.release()
        showStatus(
            when (reason) {
                Msg.BYE_PAUSED -> "Connections were paused on ${pc.name}."
                Msg.BYE_REMOVED -> "${pc.name} no longer trusts this phone."
                else -> "${pc.name} ended the session."
            },
            "OK",
        )
    }

    private fun onNotice(text: String) {
        ui.post {
            if (isFinishing || notice?.isShowing == true) return@post
            notice = AlertDialog.Builder(this).setMessage(text).setPositiveButton("OK", null).show()
        }
    }

    private fun onDisconnected() {
        conn?.close()
        conn = null
        forgetModifiers()
        controls.release()
        if (running) retry()
    }
}
