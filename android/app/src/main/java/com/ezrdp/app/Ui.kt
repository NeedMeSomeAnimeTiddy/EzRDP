package com.ezrdp.app

import android.content.Context
import android.graphics.drawable.GradientDrawable
import android.util.TypedValue
import android.view.Gravity
import android.widget.TextView

/** Small helpers so the UI can be built in code without layout files. */
fun Context.dp(v: Int): Int = (v * resources.displayMetrics.density).toInt()

fun rounded(color: Int, radius: Float, stroke: Int = 0, strokeColor: Int = 0) = GradientDrawable().apply {
    setColor(color)
    cornerRadius = radius
    if (stroke > 0) setStroke(stroke, strokeColor)
}

/** A compact pill-shaped button for the toolbars. */
fun Context.pill(label: String, onClick: () -> Unit): TextView = TextView(this).apply {
    text = label
    setTextColor(0xFFFFFFFF.toInt())
    setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
    gravity = Gravity.CENTER
    minWidth = dp(44)
    setPadding(dp(12), dp(8), dp(12), dp(8))
    background = rounded(0xCC2A2D35.toInt(), dp(18).toFloat())
    isClickable = true
    setOnClickListener { onClick() }
}

fun TextView.setActive(on: Boolean) {
    background = rounded(if (on) 0xFF4F8CFF.toInt() else 0xCC2A2D35.toInt(), context.dp(18).toFloat())
}
