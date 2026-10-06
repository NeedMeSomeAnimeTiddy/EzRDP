package com.ezrdp.app

import android.content.Context

/** All user settings, stored on the phone. */
class Settings(context: Context) {
    private val p = context.getSharedPreferences("ezrdp", Context.MODE_PRIVATE)

    var trackpad: Boolean
        get() = p.getBoolean("trackpad", true)
        set(v) = p.edit().putBoolean("trackpad", v).apply()
    var pointerSpeed: Float
        get() = p.getFloat("pointerSpeed", 1f)
        set(v) = p.edit().putFloat("pointerSpeed", v).apply()
    var scrollSpeed: Float
        get() = p.getFloat("scrollSpeed", 1f)
        set(v) = p.edit().putFloat("scrollSpeed", v).apply()
    /** false = page follows your fingers (like a phone); true = like a mouse wheel. */
    var invertScroll: Boolean
        get() = p.getBoolean("invertScroll", false)
        set(v) = p.edit().putBoolean("invertScroll", v).apply()
    var tapToClick: Boolean
        get() = p.getBoolean("tapToClick", true)
        set(v) = p.edit().putBoolean("tapToClick", v).apply()
    var showControls: Boolean
        get() = p.getBoolean("showControls", true)
        set(v) = p.edit().putBoolean("showControls", v).apply()
    /** 0 small, 1 medium, 2 large. */
    var buttonSize: Int
        get() = p.getInt("buttonSize", 1)
        set(v) = p.edit().putInt("buttonSize", v).apply()
    var controlsOpacity: Float
        get() = p.getFloat("controlsOpacity", 0.65f)
        set(v) = p.edit().putFloat("controlsOpacity", v).apply()
    var leftHanded: Boolean
        get() = p.getBoolean("leftHanded", false)
        set(v) = p.edit().putBoolean("leftHanded", v).apply()
    var haptics: Boolean
        get() = p.getBoolean("haptics", true)
        set(v) = p.edit().putBoolean("haptics", v).apply()
    var cursorSize: Float
        get() = p.getFloat("cursorSize", 1f)
        set(v) = p.edit().putFloat("cursorSize", v).apply()
    /** 0 high, 1 balanced, 2 data saver. */
    var quality: Int
        get() = p.getInt("quality", 1)
        set(v) = p.edit().putInt("quality", v).apply()
    var sound: Boolean
        get() = p.getBoolean("sound", false)
        set(v) = p.edit().putBoolean("sound", v).apply()

    /** Saved position of an on-screen control group, as fractions of the free space (0..1). */
    fun position(key: String): Pair<Float, Float>? {
        val x = p.getFloat("pos_${key}_x", -1f)
        val y = p.getFloat("pos_${key}_y", -1f)
        return if (x < 0 || y < 0) null else x to y
    }

    fun setPosition(key: String, x: Float, y: Float) =
        p.edit().putFloat("pos_${key}_x", x).putFloat("pos_${key}_y", y).apply()

    fun resetLayout() {
        val e = p.edit()
        for (k in p.all.keys) if (k.startsWith("pos_")) e.remove(k)
        e.apply()
    }
}
