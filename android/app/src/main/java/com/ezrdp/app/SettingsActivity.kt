package com.ezrdp.app

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.view.Gravity
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast

class SettingsActivity : Activity() {
    private lateinit var s: Settings
    private lateinit var col: LinearLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        s = Settings(this)
        col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(36), dp(20), dp(32))
        }
        val header = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        header.addView(pill("←") { finish() })
        header.addView(TextView(this).apply {
            text = "Settings"
            textSize = 26f
            setTextColor(Color.WHITE)
            paint.isFakeBoldText = true
            setPadding(dp(14), 0, 0, 0)
        })
        col.addView(header)

        section("Mouse")
        choice("Control mode", Settings.MODE_NAMES, s.mouseMode) { s.mouseMode = it }
        note(
            "Both: tap anything to click it, drag to move the pointer like a laptop trackpad, " +
                "or drag the pointer itself to place it exactly.\n" +
                "Trackpad: taps click wherever the pointer is.\n" +
                "Touch: the pointer is always under your finger."
        )
        slider("Pointer speed", s.pointerSpeed, 0.3f, 3f, "x") { s.pointerSpeed = it }
        toggle("Tap to click", s.tapToClick) { s.tapToClick = it }
        note("Off: tapping only moves the pointer; the on-screen buttons click.")
        slider("Pointer size", s.cursorSize, 0.6f, 2.5f, "x") { s.cursorSize = it }

        section("Scrolling")
        slider("Scroll speed", s.scrollSpeed, 0.3f, 3f, "x") { s.scrollSpeed = it }
        toggle("Reverse scroll direction", s.invertScroll) { s.invertScroll = it }
        note("Off: the page follows your fingers, like on a phone.")

        section("On-screen buttons")
        toggle("Show buttons", s.showControls) { s.showControls = it }
        choice("Button size", listOf("Small", "Medium", "Large"), s.buttonSize) { s.buttonSize = it }
        slider("Button visibility", s.controlsOpacity, 0.2f, 1f, "%") { s.controlsOpacity = it }
        toggle("Left-handed layout", s.leftHanded) {
            s.leftHanded = it
            s.resetLayout()
        }
        toggle("Vibrate on press", s.haptics) { s.haptics = it }
        action("Reset button positions") {
            s.resetLayout()
            Toast.makeText(this, "Buttons back to their default spots", Toast.LENGTH_SHORT).show()
        }
        note("To move buttons: while connected, open ☰ → Edit layout and drag them.")

        section("Picture and sound")
        choice("Quality", listOf("High", "Balanced", "Data saver"), s.quality) { s.quality = it }
        note("High is sharpest on home Wi-Fi. Data saver is best on mobile data.")
        toggle("Sound", s.sound) { s.sound = it }
        toggle("Use the PC's graphics card", s.gpuEncoder) { s.gpuEncoder = it }
        note("Makes the picture faster. Turn off if the picture looks broken or stays black.")

        setContentView(ScrollView(this).apply {
            setBackgroundColor(0xFF101114.toInt())
            addView(col)
        })
    }

    private fun section(t: String) = col.addView(TextView(this).apply {
        text = t
        textSize = 13f
        isAllCaps = true
        letterSpacing = 0.08f
        setTextColor(0xFF4F8CFF.toInt())
        setPadding(dp(2), dp(26), 0, dp(6))
    })

    private fun note(t: String) = col.addView(TextView(this).apply {
        text = t
        textSize = 13f
        setTextColor(0xFF7C838E.toInt())
        setPadding(dp(2), 0, dp(2), dp(8))
    })

    private fun label(t: String) = TextView(this).apply {
        text = t
        textSize = 16f
        setTextColor(Color.WHITE)
    }

    private fun row() = LinearLayout(this).apply {
        gravity = Gravity.CENTER_VERTICAL
        setPadding(dp(2), dp(10), dp(2), dp(10))
    }

    private fun toggle(t: String, value: Boolean, set: (Boolean) -> Unit) {
        val r = row()
        r.addView(label(t), LinearLayout.LayoutParams(0, -2, 1f))
        r.addView(Switch(this).apply {
            isChecked = value
            setOnCheckedChangeListener { _, v -> set(v) }
        })
        col.addView(r)
    }

    private fun slider(t: String, value: Float, min: Float, max: Float, unit: String, set: (Float) -> Unit) {
        val r = row()
        val shown = label("")
        fun show(v: Float) {
            shown.text = if (unit == "%") "$t  ${(v * 100).toInt()}%" else "$t  ${"%.1f".format(v)}$unit"
        }
        show(value)
        val bar = SeekBar(this).apply {
            this.max = 100
            progress = (((value - min) / (max - min)) * 100).toInt()
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(sb: SeekBar, p: Int, fromUser: Boolean) {
                    val v = min + (max - min) * p / 100f
                    show(v)
                    if (fromUser) set(v)
                }
                override fun onStartTrackingTouch(sb: SeekBar) {}
                override fun onStopTrackingTouch(sb: SeekBar) {}
            })
        }
        val c = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        c.addView(shown)
        c.addView(bar, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(6) })
        r.addView(c, LinearLayout.LayoutParams(-1, -2))
        col.addView(r)
    }

    private fun choice(t: String, options: List<String>, selected: Int, set: (Int) -> Unit) {
        val c = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(2), dp(10), dp(2), dp(10))
        }
        c.addView(label(t))
        val pills = LinearLayout(this).apply { setPadding(0, dp(8), 0, 0) }
        val views = ArrayList<TextView>()
        options.forEachIndexed { i, o ->
            val v = pill(o) {
                set(i)
                views.forEachIndexed { j, pv -> pv.setActive(j == i) }
            }
            v.setActive(i == selected)
            views.add(v)
            pills.addView(v, LinearLayout.LayoutParams(-2, -2).apply { marginEnd = dp(8) })
        }
        c.addView(pills)
        col.addView(c)
    }

    private fun action(t: String, run: () -> Unit) {
        col.addView(TextView(this).apply {
            text = t
            textSize = 16f
            setTextColor(0xFF4F8CFF.toInt())
            setPadding(dp(2), dp(12), dp(2), dp(12))
            setOnClickListener { run() }
        })
    }
}
