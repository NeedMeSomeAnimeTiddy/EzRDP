package com.ezrdp.app

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Game-style on-screen controls: mouse buttons, scroll wheel, zoom and quick buttons.
 * Each group can be dragged to a new spot in "edit layout" mode; spots are saved.
 */
@SuppressLint("ClickableViewAccessibility", "ViewConstructor")
class Controls(
    private val ctx: Context,
    private val root: FrameLayout,
    private val remote: RemoteView,
    private val conn: () -> Connection?,
    private val onKeyboard: () -> Unit,
    /** Switch to the next PC monitor. */
    private val onScreens: () -> Unit,
    private val onClick: () -> Unit,
) {
    private val s = Settings(ctx)
    private val groups = ArrayList<Group>()
    private var dragLock = false
    private var dragLockView: TextView? = null
    private var hidden = false
    var editing = false
        private set
    private var builtPortrait = false
    private val portrait get() = root.height > root.width

    /**
     * A movable group of buttons. Default spot is given as fractions of the free space.
     * Upright and sideways screens remember their own spots.
     */
    private inner class Group(name: String, val defX: Float, val defY: Float, vertical: Boolean) : LinearLayout(ctx) {
        val key = if (builtPortrait) "${name}_p" else name
        private var downX = 0f
        private var downY = 0f
        private var startX = 0f
        private var startY = 0f

        init {
            orientation = if (vertical) VERTICAL else HORIZONTAL
            gravity = Gravity.CENTER
            setPadding(ctx.dp(4), ctx.dp(4), ctx.dp(4), ctx.dp(4))
        }

        fun place() {
            val (fx, fy) = s.position(key) ?: ((if (s.leftHanded) 1f - defX else defX) to defY)
            val freeW = (root.width - width).coerceAtLeast(0)
            val freeH = (root.height - height).coerceAtLeast(0)
            x = fx * freeW
            y = fy * freeH
        }

        fun rect() = android.graphics.RectF(x, y, x + width, y + height)

        fun save() {
            val freeW = (root.width - width).coerceAtLeast(1)
            val freeH = (root.height - height).coerceAtLeast(1)
            s.setPosition(key, (x / freeW).coerceIn(0f, 1f), (y / freeH).coerceIn(0f, 1f))
        }

        override fun onInterceptTouchEvent(e: MotionEvent) = editing

        override fun onTouchEvent(e: MotionEvent): Boolean {
            if (!editing) return false
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = e.rawX; downY = e.rawY; startX = x; startY = y
                }
                MotionEvent.ACTION_MOVE -> {
                    x = (startX + e.rawX - downX).coerceIn(0f, (root.width - width).toFloat())
                    y = (startY + e.rawY - downY).coerceIn(0f, (root.height - height).toFloat())
                }
                MotionEvent.ACTION_UP -> save()
            }
            return true
        }
    }

    private fun size() = ctx.dp(when (s.buttonSize) { 0 -> 46; 2 -> 72; else -> 58 })

    private fun buzz(v: View) {
        if (s.haptics) v.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
    }

    private fun round(label: String, w: Int, h: Int = w, textSp: Float = 15f): TextView = TextView(ctx).apply {
        text = label
        setTextColor(Color.WHITE)
        textSize = textSp * (size() / ctx.dp(58).toFloat())
        gravity = Gravity.CENTER
        paint.isFakeBoldText = true
        background = shape(false)
        layoutParams = LinearLayout.LayoutParams(w, h).apply { setMargins(ctx.dp(4), ctx.dp(4), ctx.dp(4), ctx.dp(4)) }
    }

    private fun shape(active: Boolean) = GradientDrawable().apply {
        shape = GradientDrawable.RECTANGLE
        cornerRadius = 999f
        setColor(if (active) 0xEE4F8CFF.toInt() else 0xCC1E2026.toInt())
        setStroke(ctx.dp(1), 0x66FFFFFF)
    }

    private fun press(v: TextView, down: Boolean) {
        v.background = shape(down)
        if (down) buzz(v)
    }

    /** A button that holds a mouse button down for as long as it's touched. */
    private fun mouseButton(label: String, button: Int, w: Int, h: Int = w) = round(label, w, h).apply {
        setOnTouchListener { v, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    press(this, true)
                    conn()?.button(button, true)
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    press(this, false)
                    conn()?.button(button, false)
                    onClick()
                }
            }
            v.parent.requestDisallowInterceptTouchEvent(!editing)
            true
        }
    }

    private fun tapButton(label: String, w: Int, action: () -> Unit) = round(label, w).apply {
        setOnTouchListener { _, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> press(this, true)
                MotionEvent.ACTION_UP -> {
                    press(this, false)
                    action()
                }
                MotionEvent.ACTION_CANCEL -> press(this, false)
            }
            true
        }
    }

    /**
     * Acts the moment it's pressed, then keeps repeating while held (after [delay] ms,
     * every [every] ms, speeding up a little the longer it's held). A wobbly thumb doesn't stop it.
     */
    private fun repeatButton(
        label: String, w: Int, h: Int = w, delay: Long = 300, every: Long = 40,
        first: () -> Unit, again: () -> Unit,
    ) = round(label, w, h).apply {
        var count = 0
        val repeat = object : Runnable {
            override fun run() {
                again()
                count++
                postDelayed(this, (every - count * 2).coerceAtLeast(every / 2))
            }
        }
        setOnTouchListener { v, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    press(this, true)
                    count = 0
                    first()
                    postDelayed(repeat, delay)
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    press(this, false)
                    removeCallbacks(repeat)
                }
            }
            v.parent.requestDisallowInterceptTouchEvent(!editing)
            true
        }
    }

    private fun notch() = (120 * s.scrollSpeed).toInt().coerceAtLeast(40)

    /** The middle of the wheel: drag it up/down like rolling a mouse wheel. */
    private fun wheelStrip(w: Int, h: Int) = round("≡", w, h, 15f).apply {
        var lastY = 0f
        var acc = 0f
        setOnTouchListener { v, e ->
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    press(this, true)
                    lastY = e.y; acc = 0f
                }
                MotionEvent.ACTION_MOVE -> {
                    // Finger up = scroll up, like rolling a wheel forward. 20 dp = one notch.
                    acc += (lastY - e.y) / ctx.resources.displayMetrics.density * 6f * s.scrollSpeed
                    val n = acc.toInt()
                    if (n != 0) {
                        conn()?.scroll(n, 0)
                        acc -= n
                    }
                    lastY = e.y
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> press(this, false)
            }
            v.parent.requestDisallowInterceptTouchEvent(!editing)
            true
        }
    }

    private fun setDragLock(on: Boolean) {
        dragLock = on
        conn()?.button(0, on)
        dragLockView?.background = shape(on)
    }

    /** Builds (or rebuilds after settings change) all groups. */
    fun build() {
        for (g in groups) root.removeView(g)
        groups.clear()
        if (dragLock) setDragLock(false)
        builtPortrait = portrait
        val b = size()
        val tall = builtPortrait

        val mouse = Group("mouse", 1f, 1f, vertical = false)
        val left = mouseButton("L", 0, (b * 1.35).toInt())
        val middle = mouseButton("M", 2, (b * 0.8).toInt())
        val right = mouseButton("R", 1, (b * 1.35).toInt())
        val buttons = if (s.leftHanded) listOf(right, middle, left) else listOf(left, middle, right)
        buttons.forEach { mouse.addView(it) }

        // Scroll: hold ▲ or ▼ to keep scrolling, or drag the middle like a wheel.
        val wheelGroup = Group("wheel", 1f, if (tall) 0.62f else 0.45f, vertical = true)
        wheelGroup.addView(repeatButton("▲", b, delay = 350, every = 90, first = { conn()?.scroll(notch(), 0) }) { conn()?.scroll(notch(), 0) })
        wheelGroup.addView(wheelStrip(b, (b * 1.1).toInt()))
        wheelGroup.addView(repeatButton("▼", b, delay = 350, every = 90, first = { conn()?.scroll(-notch(), 0) }) { conn()?.scroll(-notch(), 0) })

        val zoom = Group("zoom", 0f, if (tall) 0.3f else 0.38f, vertical = true)
        zoom.addView(repeatButton("+", b, first = { remote.zoomBy(1.25f) }) { remote.zoomBy(1.05f) })
        zoom.addView(repeatButton("−", b, first = { remote.zoomBy(1 / 1.25f) }) { remote.zoomBy(1 / 1.05f) })
        zoom.addView(tapButton("Fit", b) { remote.resetZoom() }.apply { shrink(0.8f) })

        // Upright screens are narrow: stack these so they don't run into the mouse buttons.
        val quick = Group("quick", 0f, if (tall) 0.78f else 1f, vertical = tall)
        quick.addView(tapButton("⌨", b) { onKeyboard() })
        quick.addView(tapButton("Screen", b) { onScreens() }.apply { shrink(0.7f) })
        dragLockView = tapButton("Hold", b) { setDragLock(!dragLock) }.apply { shrink(0.75f) }
        quick.addView(dragLockView)
        quick.addView(tapButton("Hide", b) { setHidden(!hidden) }.apply {
            tag = "eye"
            shrink(0.75f)
        })

        for (g in listOf(mouse, wheelGroup, zoom, quick)) {
            groups.add(g)
            root.addView(g, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.START))
        }
        applyLook()
        root.post { placeAll() }
    }

    /** Re-place groups after rotation or resize (rotation rebuilds: different layout). */
    fun relayout() {
        if (portrait != builtPortrait) build() else placeAll()
    }

    private fun placeAll() {
        groups.forEach { it.place() }
        if (!editing) unstack()
    }

    /** Moves groups apart if any overlap (e.g. on a small screen), mouse buttons stay put. */
    private fun unstack() {
        val gap = ctx.dp(4).toFloat()
        for (i in 1 until groups.size) {
            val g = groups[i]
            if (g.visibility != View.VISIBLE || g.width == 0) continue
            repeat(groups.size) {
                val hit = groups.take(i).firstOrNull { o ->
                    o.visibility == View.VISIBLE && android.graphics.RectF.intersects(o.rect(), g.rect())
                } ?: return@repeat
                val above = hit.y - g.height - gap
                val below = hit.y + hit.height + gap
                g.y = if (above >= 0) above else below.coerceAtMost((root.height - g.height).toFloat())
            }
        }
    }

    private fun TextView.shrink(f: Float) = setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, textSize * f)

    private fun setHidden(h: Boolean) {
        hidden = h
        groups.flatMap { g -> (0 until g.childCount).map { g.getChildAt(it) } }
            .firstOrNull { it.tag == "eye" }
            ?.let { (it as TextView).text = if (h) "Show" else "Hide" }
        applyLook()
    }

    private fun applyLook() {
        val show = s.showControls
        for (g in groups) {
            g.visibility = if (show) View.VISIBLE else View.GONE
            g.alpha = s.controlsOpacity
            g.background = if (editing) GradientDrawable().apply {
                cornerRadius = ctx.dp(14).toFloat()
                setStroke(ctx.dp(2), 0xFF4F8CFF.toInt(), ctx.dp(6).toFloat(), ctx.dp(4).toFloat())
                setColor(0x334F8CFF)
            } else null
            // When hidden, only the Show button stays, faint, so controls can be brought back.
            for (i in 0 until g.childCount) {
                val c = g.getChildAt(i)
                c.visibility = if (hidden && c.tag != "eye" && !editing) View.INVISIBLE else View.VISIBLE
            }
            if (hidden && !editing && g.key == "quick") g.alpha = 0.3f
        }
    }

    fun setEditing(on: Boolean) {
        editing = on
        if (on) hidden = false
        applyLook()
        if (!on) groups.forEach { it.save() }
    }

    /** Lets go of a held button (e.g. when disconnecting). */
    fun release() {
        if (dragLock) setDragLock(false)
    }
}
