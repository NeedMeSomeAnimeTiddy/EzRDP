package com.ezrdp.app

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.os.SystemClock
import android.view.Choreographer
import android.view.MotionEvent
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.ViewConfiguration
import android.widget.FrameLayout
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min

/**
 * Shows the PC screen, draws the mouse pointer, handles zoom and turns touches into mouse input.
 *
 * Three ways to control (Settings → Control mode):
 *  - Both (default): tap = the pointer jumps there and clicks; drag = move the pointer like a
 *    trackpad; drag starting on the pointer = move it exactly with your finger;
 *    tap then drag = drag things; long press = right click there.
 *  - Trackpad: drag = move pointer, tap = click where the pointer is, tap then drag = drag.
 *  - Touch: the pointer is always under your finger; tap = click, drag = drag.
 * In every mode: two-finger tap = right click, two-finger drag = scroll, pinch = zoom.
 *
 * The video is drawn by a SurfaceView (lower delay than a TextureView). Zoom and pan just
 * move and resize that view; the phone's display hardware does the scaling.
 */
@SuppressLint("ViewConstructor")
class RemoteView(context: Context, private val conn: () -> Connection?) :
    FrameLayout(context), SurfaceHolder.Callback {

    var onSurface: (Surface?) -> Unit = {}
    /** Called after any click or key, so one-shot modifier keys can be released. */
    var onAction: () -> Unit = {}
    /** 0 = both, 1 = trackpad, 2 = touch. */
    var mode = Settings.MODE_BOTH
    var pointerSpeed = 1f
    var scrollSpeed = 1f
    var invertScroll = false
    var tapToClick = true
    var cursorScale = 1f
    var haptics = true

    fun apply(st: Settings) {
        mode = st.mouseMode
        pointerSpeed = st.pointerSpeed
        scrollSpeed = st.scrollSpeed
        invertScroll = st.invertScroll
        tapToClick = st.tapToClick
        cursorScale = st.cursorSize
        haptics = st.haptics
        invalidate()
    }

    private val video = SurfaceView(context)
    private val dp = resources.displayMetrics.density
    private val slop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()

    // Video size in pixels and the current zoom / pan.
    private var videoW = 0
    private var videoH = 0
    private var zoom = 1f
    private var centerX = 0f
    private var centerY = 0f
    private var scale = 1f
    private var tx = 0f
    private var ty = 0f

    // Pointer position in video pixels.
    private var cursorX = 0f
    private var cursorY = 0f
    private var cursorVisible = true
    private var lastLocalMove = 0L

    init {
        video.holder.addCallback(this)
        addView(video, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        setWillNotDraw(false)
    }

    // ---------- Surface ----------

    override fun surfaceCreated(holder: SurfaceHolder) = onSurface(holder.surface)

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, w: Int, h: Int) {}

    override fun surfaceDestroyed(holder: SurfaceHolder) = onSurface(null)

    override fun onLayout(changed: Boolean, l: Int, t: Int, r: Int, b: Int) {
        placeVideo()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        applyTransform()
    }

    /** Puts the video view where the zoom/pan says the picture is. */
    private fun placeVideo() {
        if (videoW == 0 || width == 0) {
            video.layout(0, 0, width, height)
            return
        }
        val l = Math.round(tx)
        val t = Math.round(ty)
        video.layout(l, t, l + Math.round(videoW * scale), t + Math.round(videoH * scale))
    }

    // ---------- Video geometry ----------

    fun setVideoSize(w: Int, h: Int) {
        if (w == videoW && h == videoH) return
        videoW = w
        videoH = h
        video.holder.setFixedSize(w, h)
        zoom = 1f
        centerX = w / 2f
        centerY = h / 2f
        cursorX = w / 2f
        cursorY = h / 2f
        applyTransform()
    }

    fun setRemoteCursor(nx: Float, ny: Float, visible: Boolean) {
        cursorVisible = visible
        // While the finger is moving the pointer we trust our own position (no jitter).
        if (SystemClock.uptimeMillis() - lastLocalMove > 400) {
            cursorX = nx * videoW
            cursorY = ny * videoH
        }
        invalidate()
    }

    fun resetZoom() {
        zoom = 1f
        applyTransform()
    }

    val zoomed get() = zoom > 1.01f

    /** Zooms in or out by [factor], around the pointer if it's on screen, else around the middle. */
    fun zoomBy(factor: Float) {
        if (videoW == 0 || width == 0) return
        var px = tx + cursorX * scale
        var py = ty + cursorY * scale
        var vx = cursorX
        var vy = cursorY
        if (px < 0 || px > width || py < 0 || py > height || !cursorVisible) {
            px = width / 2f
            py = height / 2f
            vx = toVideoX(px)
            vy = toVideoY(py)
        }
        zoom = (zoom * factor).coerceIn(1f, 8f)
        val sc = baseScale() * zoom
        centerX = (width / 2f - (px - vx * sc)) / sc
        centerY = (height / 2f - (py - vy * sc)) / sc
        applyTransform()
    }

    private fun baseScale() = min(width.toFloat() / videoW, height.toFloat() / videoH)

    private fun applyTransform() {
        val w = width.toFloat()
        val h = height.toFloat()
        if (videoW == 0 || w == 0f || h == 0f) return
        scale = baseScale() * zoom
        // Keep the picture filling the screen when zoomed; centred otherwise.
        val halfW = w / 2 / scale
        val halfH = h / 2 / scale
        centerX = if (videoW <= halfW * 2) videoW / 2f else centerX.coerceIn(halfW, videoW - halfW)
        centerY = if (videoH <= halfH * 2) videoH / 2f else centerY.coerceIn(halfH, videoH - halfH)
        tx = w / 2 - centerX * scale
        ty = h / 2 - centerY * scale
        placeVideo()
        invalidate()
    }

    private fun toVideoX(x: Float) = (x - tx) / scale
    private fun toVideoY(y: Float) = (y - ty) / scale

    /** When zoomed in, scroll the view so the pointer stays visible. */
    private fun follow() {
        if (!zoomed) return
        val margin = 40 * dp
        val vx = tx + cursorX * scale
        val vy = ty + cursorY * scale
        if (vx < margin) centerX -= (margin - vx) / scale
        if (vx > width - margin) centerX += (vx - width + margin) / scale
        if (vy < margin) centerY -= (margin - vy) / scale
        if (vy > height - margin) centerY += (vy - height + margin) / scale
        applyTransform()
    }

    // ---------- Edge panning ----------
    // While zoomed and a finger is down, pushing the pointer into the screen edge keeps the view
    // gliding that way (faster the closer to the edge), carrying the pointer along.

    private var fingerDown = false
    private var lastFrameNanos = 0L
    private var edgeRunning = false

    private val edgeFrame = object : Choreographer.FrameCallback {
        override fun doFrame(now: Long) {
            if (!fingerDown || !zoomed || videoW == 0) {
                edgeRunning = false
                return
            }
            val dt = if (lastFrameNanos == 0L) 0f else ((now - lastFrameNanos) / 1e9f).coerceAtMost(0.05f)
            lastFrameNanos = now
            val zone = 56 * dp
            val px = tx + cursorX * scale
            val py = ty + cursorY * scale
            fun push(pos: Float, size: Float) = when {
                pos < zone -> -(1 - (pos / zone).coerceIn(0f, 1f))
                pos > size - zone -> 1 - ((size - pos) / zone).coerceIn(0f, 1f)
                else -> 0f
            }
            val ex = push(px, width.toFloat())
            val ey = push(py, height.toFloat())
            if ((ex != 0f || ey != 0f) && dt > 0f) {
                // Up to 1.6 screens per second, in screen pixels, turned into video pixels.
                val speed = 1.6f * maxOf(width, height) * dt / scale
                val oldX = centerX
                val oldY = centerY
                centerX += ex * ex * Math.signum(ex) * speed
                centerY += ey * ey * Math.signum(ey) * speed
                applyTransform()
                val dx = centerX - oldX
                val dy = centerY - oldY
                if (dx != 0f || dy != 0f) {
                    val nx = (cursorX + dx).coerceIn(0f, videoW - 1f)
                    val ny = (cursorY + dy).coerceIn(0f, videoH - 1f)
                    sendPointer(nx, ny)
                }
            }
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    private fun startEdgePan() {
        if (edgeRunning || !zoomed) return
        edgeRunning = true
        lastFrameNanos = 0L
        Choreographer.getInstance().postFrameCallback(edgeFrame)
    }

    // ---------- Pointer drawing ----------

    private val arrow = Path().apply {
        moveTo(0f, 0f); lineTo(0f, 17f); lineTo(4.5f, 13f); lineTo(7.5f, 19.5f)
        lineTo(10f, 18.5f); lineTo(7f, 12f); lineTo(12.5f, 12f); close()
    }
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = 0xFFFFFFFF.toInt() }
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFF000000.toInt(); style = Paint.Style.STROKE; strokeWidth = 1.2f; strokeJoin = Paint.Join.ROUND
    }

    override fun dispatchDraw(canvas: Canvas) {
        super.dispatchDraw(canvas)
        if (videoW == 0 || !cursorVisible) return
        canvas.save()
        canvas.translate(tx + cursorX * scale, ty + cursorY * scale)
        val s = dp * 1.1f * cursorScale
        canvas.scale(s, s)
        canvas.drawPath(arrow, fill)
        canvas.drawPath(arrow, stroke)
        canvas.restore()
    }

    // ---------- Touch ----------

    private enum class Two { NONE, UNDECIDED, SCROLL, PINCH }

    /** What a one-finger drag is doing. */
    private enum class Drag { NONE, RELATIVE, ABSOLUTE }

    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var downTime = 0L
    private var moved = false
    private var dragging = false
    private var drag = Drag.NONE
    /** Finger started on the pointer: it follows the finger exactly (keeping this offset). */
    private var grabbed = false
    private var grabDx = 0f
    private var grabDy = 0f
    private var longPressed = false
    private var multi = false
    private var lastTapTime = 0L
    private var lastTapX = 0f
    private var lastTapY = 0f

    private var two = Two.NONE
    private var twoStart = 0L
    private var startSpan = 0f
    private var startZoom = 1f
    private var startFocusX = 0f
    private var startFocusY = 0f
    private var focusVideoX = 0f
    private var focusVideoY = 0f
    private var lastFocusY = 0f
    private var lastFocusX = 0f
    private var scrollAccY = 0f
    private var scrollAccX = 0f

    private val trackpad get() = mode == Settings.MODE_TRACKPAD
    private val touch get() = mode == Settings.MODE_TOUCH

    private fun buzz(kind: Int) {
        if (haptics) performHapticFeedback(kind)
    }

    private val longPress = Runnable {
        if (moved || multi || dragging) return@Runnable
        longPressed = true
        if (!trackpad) moveAbsTo(downX, downY)
        conn()?.click(1)
        buzz(android.view.HapticFeedbackConstants.LONG_PRESS)
        onAction()
    }

    /** Moves the pointer to a spot in video pixels and tells the PC. */
    private fun sendPointer(x: Float, y: Float) {
        if (videoW == 0) return
        cursorX = x.coerceIn(0f, videoW - 1f)
        cursorY = y.coerceIn(0f, videoH - 1f)
        lastLocalMove = SystemClock.uptimeMillis()
        conn()?.mouseAbs(cursorX / videoW, cursorY / videoH)
        invalidate()
    }

    private fun moveAbsTo(x: Float, y: Float) = sendPointer(toVideoX(x), toVideoY(y))

    private fun moveRel(dxPx: Float, dyPx: Float) {
        if (videoW == 0) return
        // Faster finger = bigger jump, slow finger = precise. Zooming in makes it finer still.
        val dist = hypot(dxPx, dyPx) / dp
        val accel = (0.9f + min(dist / 12f, 1.8f)) * pointerSpeed
        val dx = dxPx / scale * accel
        val dy = dyPx / scale * accel
        val nx = (cursorX + dx).coerceIn(0f, videoW - 1f)
        val ny = (cursorY + dy).coerceIn(0f, videoH - 1f)
        conn()?.mouseRel((nx - cursorX) / videoW, (ny - cursorY) / videoH)
        cursorX = nx
        cursorY = ny
        lastLocalMove = SystemClock.uptimeMillis()
        follow()
        invalidate()
    }

    private fun nearPointer(x: Float, y: Float): Boolean {
        val px = tx + cursorX * scale
        val py = ty + cursorY * scale
        // The arrow's tip is at the top-left; aim a little below-right of it.
        return cursorVisible && hypot(x - (px + 6 * dp), y - (py + 10 * dp)) < 34 * dp
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(e: MotionEvent): Boolean {
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = e.x; downY = e.y; lastX = e.x; lastY = e.y
                downTime = SystemClock.uptimeMillis()
                moved = false; longPressed = false; multi = false; dragging = false
                drag = Drag.NONE; grabbed = false
                two = Two.NONE
                fingerDown = true
                val quick = downTime - lastTapTime < 300 && hypot(e.x - lastTapX, e.y - lastTapY) < 50 * dp
                when {
                    touch -> moveAbsTo(e.x, e.y)
                    quick && (tapToClick || !trackpad) -> {
                        // Tap then hold: press the button and drag.
                        dragging = true
                        conn()?.button(0, true)
                    }
                    !trackpad && nearPointer(e.x, e.y) -> {
                        grabbed = true
                        grabDx = tx + cursorX * scale - e.x
                        grabDy = ty + cursorY * scale - e.y
                    }
                }
                postDelayed(longPress, 520)
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                removeCallbacks(longPress)
                if (dragging) {
                    conn()?.button(0, false)
                    dragging = false
                }
                multi = true
                if (e.pointerCount == 2) startTwo(e) else two = Two.NONE
            }

            MotionEvent.ACTION_MOVE -> {
                if (multi) {
                    if (e.pointerCount == 2 && two != Two.NONE) moveTwo(e)
                    return true
                }
                if (!moved && hypot(e.x - downX, e.y - downY) > slop) {
                    moved = true
                    removeCallbacks(longPress)
                    if (touch && !longPressed && !dragging) {
                        dragging = true
                        conn()?.button(0, true)
                    }
                    drag = when {
                        touch || grabbed -> Drag.ABSOLUTE
                        dragging && !trackpad -> Drag.ABSOLUTE
                        else -> Drag.RELATIVE
                    }
                }
                if (moved && !longPressed) {
                    when (drag) {
                        Drag.ABSOLUTE -> {
                            if (grabbed) moveAbsTo(e.x + grabDx, e.y + grabDy) else moveAbsTo(e.x, e.y)
                            follow()
                        }
                        Drag.RELATIVE -> moveRel(e.x - lastX, e.y - lastY)
                        Drag.NONE -> {}
                    }
                    startEdgePan()
                }
                lastX = e.x; lastY = e.y
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (two == Two.UNDECIDED && SystemClock.uptimeMillis() - twoStart < 300) {
                    conn()?.click(1) // two-finger tap = right click
                    buzz(android.view.HapticFeedbackConstants.CONTEXT_CLICK)
                    onAction()
                }
                two = Two.NONE
            }

            MotionEvent.ACTION_UP -> {
                removeCallbacks(longPress)
                fingerDown = false
                val now = SystemClock.uptimeMillis()
                if (dragging) {
                    conn()?.button(0, false)
                    dragging = false
                    onAction()
                    lastTapTime = 0
                } else if (!moved && !multi && !longPressed && now - downTime < 300) {
                    // A tap. In "both" mode the pointer first jumps to the finger.
                    if (mode == Settings.MODE_BOTH && !grabbed) moveAbsTo(e.x, e.y)
                    if (tapToClick || touch) {
                        conn()?.click(0)
                        onAction()
                    }
                    lastTapTime = now; lastTapX = e.x; lastTapY = e.y
                }
            }

            MotionEvent.ACTION_CANCEL -> {
                removeCallbacks(longPress)
                fingerDown = false
                if (dragging) conn()?.button(0, false)
                dragging = false
                two = Two.NONE
            }
        }
        return true
    }

    private fun span(e: MotionEvent) = hypot(e.getX(0) - e.getX(1), e.getY(0) - e.getY(1))
    private fun focusX(e: MotionEvent) = (e.getX(0) + e.getX(1)) / 2
    private fun focusY(e: MotionEvent) = (e.getY(0) + e.getY(1)) / 2

    private fun startTwo(e: MotionEvent) {
        two = Two.UNDECIDED
        twoStart = SystemClock.uptimeMillis()
        startSpan = span(e)
        startZoom = zoom
        startFocusX = focusX(e); startFocusY = focusY(e)
        lastFocusX = startFocusX; lastFocusY = startFocusY
        focusVideoX = toVideoX(startFocusX); focusVideoY = toVideoY(startFocusY)
        scrollAccX = 0f; scrollAccY = 0f
    }

    private fun moveTwo(e: MotionEvent) {
        val fx = focusX(e)
        val fy = focusY(e)
        if (two == Two.UNDECIDED) {
            if (abs(span(e) - startSpan) > 28 * dp) {
                two = Two.PINCH
                // Restart from here so the zoom doesn't jump.
                startSpan = span(e); startZoom = zoom
                focusVideoX = toVideoX(fx); focusVideoY = toVideoY(fy)
            } else if (hypot(fx - startFocusX, fy - startFocusY) > slop) {
                two = Two.SCROLL
            }
        }
        when (two) {
            Two.PINCH -> {
                zoom = (startZoom * span(e) / startSpan).coerceIn(1f, 8f)
                val sc = baseScale() * zoom
                // Keep the spot between the fingers under the fingers.
                centerX = (width / 2f - (fx - focusVideoX * sc)) / sc
                centerY = (height / 2f - (fy - focusVideoY * sc)) / sc
                applyTransform()
            }
            Two.SCROLL -> {
                // 30 dp of finger travel = one mouse-wheel notch (120). Content follows the fingers.
                val dir = if (invertScroll) -1f else 1f
                scrollAccY += (fy - lastFocusY) / dp * 4f * scrollSpeed * dir
                scrollAccX += -(fx - lastFocusX) / dp * 4f * scrollSpeed * dir
                val sy = scrollAccY.toInt()
                val sx = scrollAccX.toInt()
                if (sy != 0 || sx != 0) {
                    conn()?.scroll(sy, sx)
                    scrollAccY -= sy
                    scrollAccX -= sx
                }
            }
            else -> {}
        }
        lastFocusX = fx; lastFocusY = fy
    }
}
