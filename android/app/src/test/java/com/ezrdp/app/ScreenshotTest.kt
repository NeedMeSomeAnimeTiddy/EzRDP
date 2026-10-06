package com.ezrdp.app

import android.app.Activity
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Looper
import androidx.test.core.app.ApplicationProvider
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/** Renders the app's screens to PNGs (in /screenshots) so the layout can be checked without a phone. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34])
class ScreenshotTest {
    private fun shot(a: Activity, name: String) {
        repeat(3) { shadowOf(Looper.getMainLooper()).idle() }
        val v = a.window.decorView
        val bmp = Bitmap.createBitmap(v.width, v.height, Bitmap.Config.ARGB_8888)
        v.draw(Canvas(bmp))
        val dir = File(System.getProperty("shots.dir") ?: "screenshots").apply { mkdirs() }
        File(dir, "$name.png").outputStream().use { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    @Test
    @Config(qualifiers = "w411dp-h891dp-port-xxhdpi")
    fun main() {
        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        PcStore(ctx).save(SavedPc("00".repeat(16), "GAMING-PC", ByteArray(16), ByteArray(32), internet = "81.2.69.160"))
        shot(Robolectric.buildActivity(MainActivity::class.java).setup().get(), "main")
    }

    @Test
    @Config(qualifiers = "w411dp-h891dp-port-xxhdpi")
    fun settings() = shot(Robolectric.buildActivity(SettingsActivity::class.java).setup().get(), "settings")

    @Test
    @Config(qualifiers = "w891dp-h411dp-land-xxhdpi")
    fun session() {
        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        PcStore(ctx).save(SavedPc("11".repeat(16), "GAMING-PC", ByteArray(16), ByteArray(32), home = "127.0.0.9"))
        val i = Intent(ctx, SessionActivity::class.java).putExtra("pc", "11".repeat(16))
        val a = Robolectric.buildActivity(SessionActivity::class.java, i).setup().get()
        shot(a, "session")
    }
}
