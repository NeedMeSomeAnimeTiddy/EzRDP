package com.ezrdp.app

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.SurfaceTexture
import android.os.SystemClock
import android.view.MotionEvent
import android.view.Surface
import android.view.TextureView
import android.view.ViewConfiguration
import android.widget.FrameLayout
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min

/**
 * Shows the PC screen, draws the mouse pointer, handles zoom and turns touches into mouse input.
 *
 * Trackpad mode (default): drag = move pointer, tap = click, tap then drag = hold-and-drag,
 * two-finger tap or long press = right click, two-finger drag = scroll, pinch = zoom.
 * Touch mode: the pointer jumps to your finger; tap = click there, drag = drag, long press = right click.
 */
@SuppressLint("ViewConstructor")
class RemoteView(context: Context, private val conn: () -> Connection?) :
    FrameLayout(context), TextureView.SurfaceTextureListener {

    var onSurface: (Surface?) -> Unit = {}
    /** Called after any click or key, so one-shot modifier keys can be released. */
    var onAction: () -> Unit = {}
    var trackpad = true
    var pointerSpeed = 1f
    var scrollSpeed = 1f
    var invertScroll = false
    var tapToClick = true
    var cursorScale = 1f
    var haptics = true

    fun apply(st: Settings) {
        trackpad = st.trackpad
        pointerSpeed = st.pointerSpeed
        scrollSpeed = st.scrollSpeed
        invertScroll = st.invertScroll
        tapToClick = st.tapToClick
        cursorScale = st.cursorSize
        haptics = st.haptics
        invalidate()
    }

    private val texture = TextureView(context)
    private var surface: Surface? = null
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
    private val matrix = Matrix()

    // Pointer position in video pixels.
    private var cursorX = 0f
    private var cursorY = 0f
    private var cursorVisible = true
    private var lastLocalMove = 0L

    init {
        texture.surfaceTextureListener = this
        addView(texture, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        setWillNotDraw(false)
    }

    // ---------- Surface ----------

    override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
        surface = Surface(st)
        onSurface(surface)
        applyTransform()
    }

    override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) = applyTransform()

    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
        onSurface(null)
        surface?.release()
        surface = null
        return true
    }

    override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        applyTransform()
    }

    // ---------- Video geometry ----------

    fun setVideoSize(w: Int, h: Int) {
        if (w == videoW && h == videoH) return
        videoW = w
        videoH = h
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

    /** Zooms in or out by [factor], keeping the mouse pointer where it is on screen. */
    fun zoomBy(factor: Float) {
        if (videoW == 0) return
        val px = tx + cursorX * scale
        val py = ty + cursorY * scale
        zoom = (zoom * factor).coerceIn(1f, 6f)
        val sc = min(width.toFloat() / videoW, height.toFloat() / videoH) * zoom
        centerX = (width / 2f - (px - cursorX * sc)) / sc
        centerY = (height / 2f - (py - cursorY * sc)) / sc
        applyTransform()
    }

    private fun applyTransform() {
        val w = width.toFloat()
        val h = height.toFloat()
        if (videoW == 0 || w == 0f || h == 0f) return
        scale = min(w / videoW, h / videoH) * zoom
        // Keep the picture filling the screen when zoomed; centred otherwise.
        val halfW = w / 2 / scale
        val halfH = h / 2 / scale
        centerX = if (videoW <= halfW * 2) videoW / 2f else centerX.coerceIn(halfW, videoW - halfW)
        centerY = if (videoH <= halfH * 2) videoH / 2f else centerY.coerceIn(halfH, videoH - halfH)
        tx = w / 2 - centerX * scale
        ty = h / 2 - centerY * scale
        // TextureView stretches the video to the view; undo that and apply our own scale.
        matrix.setScale(scale * videoW / w, scale * videoH / h)
        matrix.postTranslate(tx, ty)
        texture.setTransform(matrix)
        invalidate()
    }

    private fun toVideoX(x: Float) = (x - tx) / scale
    private fun toVideoY(y: Float) = (y - ty) / scale

    /** When zoomed in, scroll the view so the pointer stays visible. */
    private fun follow() {
        if (zoom <= 1f) return
        val margin = 60 * dp
        val vx = tx + cursorX * scale
        val vy = ty + cursorY * scale
        if (vx < margin) centerX -= (margin - vx) / scale
        if (vx > width - margin) centerX += (vx - width + margin) / scale
        if (vy < margin) centerY -= (margin - vy) / scale
        if (vy > height - margin) centerY += (vy - height + margin) / scale
        applyTransform()
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

    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var downTime = 0L
    private var moved = false
    private var dragging = false
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

    private val longPress = Runnable {
        if (moved || multi || dragging) return@Runnable
        longPressed = true
        if (!trackpad) moveAbsTo(downX, downY)
        conn()?.click(1)
        if (haptics) performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
        onAction()
    }

    private fun moveAbsTo(x: Float, y: Float) {
        if (videoW == 0) return
        cursorX = toVideoX(x).coerceIn(0f, videoW - 1f)
        cursorY = toVideoY(y).coerceIn(0f, videoH - 1f)
        lastLocalMove = SystemClock.uptimeMillis()
        conn()?.mouseAbs(cursorX / videoW, cursorY / videoH)
        invalidate()
    }

    private fun moveRel(dxPx: Float, dyPx: Float) {
        if (videoW == 0) return
        // Faster finger = bigger jump, slow finger = precise.
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

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(e: MotionEvent): Boolean {
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = e.x; downY = e.y; lastX = e.x; lastY = e.y
                downTime = SystemClock.uptimeMillis()
                moved = false; longPressed = false; multi = false; dragging = false
                two = Two.NONE
                if (trackpad && tapToClick) {
                    val quick = downTime - lastTapTime < 280
                    if (quick && hypot(e.x - lastTapX, e.y - lastTapY) < 50 * dp) {
                        // Tap then hold: press the button and drag.
                        dragging = true
                        conn()?.button(0, true)
                    }
                } else {
                    moveAbsTo(e.x, e.y)
                }
                postDelayed(longPress, 550)
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                removeCallbacks(longPress)
                if (dragging && trackpad) {
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
                    if (!trackpad && !longPressed) {
                        dragging = true
                        conn()?.button(0, true)
                    }
                }
                if (moved && !longPressed) {
                    if (trackpad) moveRel(e.x - lastX, e.y - lastY) else moveAbsTo(e.x, e.y)
                }
                lastX = e.x; lastY = e.y
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (two == Two.UNDECIDED && SystemClock.uptimeMillis() - twoStart < 300) {
                    conn()?.click(1) // two-finger tap = right click
                    onAction()
                }
                two = Two.NONE
            }

            MotionEvent.ACTION_UP -> {
                removeCallbacks(longPress)
                val now = SystemClock.uptimeMillis()
                if (dragging) {
                    conn()?.button(0, false)
                    dragging = false
                    onAction()
                    lastTapTime = 0
                } else if (!moved && !multi && !longPressed && now - downTime < 300 && (tapToClick || !trackpad)) {
                    conn()?.click(0)
                    onAction()
                    lastTapTime = now; lastTapX = e.x; lastTapY = e.y
                }
            }

            MotionEvent.ACTION_CANCEL -> {
                removeCallbacks(longPress)
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
                zoom = (startZoom * span(e) / startSpan).coerceIn(1f, 6f)
                val base = min(width.toFloat() / videoW, height.toFloat() / videoH)
                val sc = base * zoom
                // Keep the spot between the fingers under the fingers.
                centerX = width / 2f - (fx - focusVideoX * sc)
                centerX /= sc
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
