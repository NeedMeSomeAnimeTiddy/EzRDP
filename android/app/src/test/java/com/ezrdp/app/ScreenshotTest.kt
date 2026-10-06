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
    fun session() = shot(openSession(), "session")

    private fun openSession(): SessionActivity {
        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        PcStore(ctx).save(SavedPc("11".repeat(16), "GAMING-PC", ByteArray(16), ByteArray(32), home = "127.0.0.9"))
        val i = Intent(ctx, SessionActivity::class.java).putExtra("pc", "11".repeat(16))
        return Robolectric.buildActivity(SessionActivity::class.java, i).setup().get()
    }

    @Test
    @Config(qualifiers = "w411dp-h891dp-port-xxhdpi")
    fun sessionUpright() = shot(openSession(), "session_upright")

    /** The longest error message, on the shortest screen (sideways phone). */
    @Test
    @Config(qualifiers = "w780dp-h360dp-land-xxhdpi")
    fun sessionError() {
        val a = openSession()
        val msg = "Can't reach GAMING-PC.\n\n" +
            "• Is the PC on, with EzRDP running (icon next to the clock)?\n" +
            "• From outside your home, your router must let EzRDP in: check \"Connection info\" in the EzRDP tray menu on the PC.\n" +
            "• At home, make sure the phone is on the same Wi-Fi."
        SessionActivity::class.java.getDeclaredMethod("fail", String::class.java).apply { isAccessible = true }.invoke(a, msg)
        shot(a, "session_error")
    }

    @Test
    @Config(qualifiers = "w411dp-h891dp-port-xxhdpi")
    fun share() {
        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        PcStore(ctx).save(SavedPc("22".repeat(16), "GAMING-PC", ByteArray(16), ByteArray(32)))
        val i = Intent(ctx, ShareActivity::class.java).putExtra("pc", "22".repeat(16))
        shot(Robolectric.buildActivity(ShareActivity::class.java, i).setup().get(), "share")
    }
}
