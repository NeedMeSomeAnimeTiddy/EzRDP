package com.ezrdp.app

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Context
import android.graphics.Path
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.provider.Settings as SystemSettings
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import kotlin.math.hypot

/**
 * Lets the PC tap, swipe, type and press Back/Home on this phone while it's shared.
 * Android only allows this through an accessibility service, which the person turns on once
 * in the phone's settings. It does nothing unless this phone is sharing its screen with a PC.
 */
class ControlService : AccessibilityService() {

    companion object {
        @Volatile var instance: ControlService? = null
            private set

        fun isEnabled(ctx: Context): Boolean {
            val list = SystemSettings.Secure.getString(
                ctx.contentResolver, SystemSettings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            ) ?: return false
            return list.split(':').any { it.startsWith(ctx.packageName + "/") }
        }
    }

    override fun onServiceConnected() {
        instance = this
    }

    override fun onDestroy() {
        if (instance == this) instance = null
        super.onDestroy()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}
    override fun onInterrupt() {}

    // ---------- Screen size ----------

    private fun screenSize(): Pair<Int, Int> {
        val wm = getSystemService(WindowManager::class.java)
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val b = wm.currentWindowMetrics.bounds
            b.width() to b.height()
        } else {
            val m = android.util.DisplayMetrics()
            @Suppress("DEPRECATION")
            wm.defaultDisplay.getRealMetrics(m)
            m.widthPixels to m.heightPixels
        }
    }

    // ---------- Touch ----------

    private val points = ArrayList<FloatArray>() // x, y, time since down (ms)
    private var downAt = 0L

    /** action 0 = down, 1 = move, 2 = up; x/y are 0..1 of the screen. */
    fun touch(action: Int, fx: Float, fy: Float) {
        val (w, h) = screenSize()
        val x = (fx * w).coerceIn(0f, w - 1f)
        val y = (fy * h).coerceIn(0f, h - 1f)
        val now = SystemClock.uptimeMillis()
        when (action) {
            0 -> {
                points.clear()
                downAt = now
                points.add(floatArrayOf(x, y, 0f))
            }
            1 -> if (points.isNotEmpty()) {
                val last = points.last()
                if (hypot(x - last[0], y - last[1]) > 3f) points.add(floatArrayOf(x, y, (now - downAt).toFloat()))
            }
            2 -> if (points.isNotEmpty()) {
                points.add(floatArrayOf(x, y, (now - downAt).toFloat()))
                play(ArrayList(points), now - downAt)
                points.clear()
            }
        }
    }

    /** Replays a finished touch: a tap, a long press, or a swipe along the mouse's path. */
    private fun play(pts: List<FloatArray>, heldMs: Long) {
        val first = pts.first()
        val moved = pts.any { hypot(it[0] - first[0], it[1] - first[1]) > 12f }
        val path = Path().apply { moveTo(first[0], first[1]) }
        val duration: Long
        if (!moved) {
            // Short click = tap; holding the mouse button = long press.
            duration = if (heldMs > 450) heldMs.coerceAtMost(3000) else 40
        } else {
            for (p in pts.drop(1)) path.lineTo(p[0], p[1])
            // Same speed as the mouse moved, so flings and slow drags both work.
            duration = heldMs.coerceIn(60, 8000)
        }
        val stroke = GestureDescription.StrokeDescription(path, 0, duration)
        dispatchGesture(GestureDescription.Builder().addStroke(stroke).build(), null, null)
    }

    /** Scroll-wheel: a quick swipe from (x, y) by (dx, dy), all in 0..1 of the screen. */
    fun swipe(fx: Float, fy: Float, dx: Float, dy: Float) {
        val (w, h) = screenSize()
        val x = (fx * w).coerceIn(1f, w - 2f)
        val y = (fy * h).coerceIn(1f, h - 2f)
        val ex = (x + dx * w).coerceIn(1f, w - 2f)
        val ey = (y + dy * h).coerceIn(1f, h - 2f)
        if (hypot(ex - x, ey - y) < 4f) return
        val path = Path().apply { moveTo(x, y); lineTo(ex, ey) }
        // Slow enough not to fling: the page moves by the swipe length and stops.
        val stroke = GestureDescription.StrokeDescription(path, 0, 160)
        dispatchGesture(GestureDescription.Builder().addStroke(stroke).build(), null, null)
    }

    // ---------- Buttons ----------

    fun nav(what: Int) {
        when (what) {
            Msg.NAV_BACK -> performGlobalAction(GLOBAL_ACTION_BACK)
            Msg.NAV_HOME -> performGlobalAction(GLOBAL_ACTION_HOME)
            Msg.NAV_RECENTS -> performGlobalAction(GLOBAL_ACTION_RECENTS)
            Msg.NAV_NOTIFICATIONS -> performGlobalAction(GLOBAL_ACTION_NOTIFICATIONS)
            Msg.NAV_BACKSPACE -> editText { it.dropLast(1) }
            Msg.NAV_ENTER -> enter()
        }
    }

    // ---------- Typing ----------

    private fun focused(): AccessibilityNodeInfo? =
        rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.takeIf { it.isEditable }

    /** Changes the text of the box that has focus, keeping the cursor at the end. */
    private fun editText(change: (String) -> String) {
        val node = focused() ?: return
        val current = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && node.isShowingHintText) "" else node.text?.toString() ?: ""
        val text = change(current)
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        val sel = Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, text.length)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, text.length)
        }
        node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, sel)
    }

    fun type(s: String) = editText { it + s }

    private fun enter() {
        val node = focused() ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
        ) return
        if (node.isMultiLine) editText { it + "\n" }
    }
}
