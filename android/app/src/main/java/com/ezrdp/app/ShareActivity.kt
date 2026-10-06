package com.ezrdp.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings as SystemSettings
import android.view.Gravity
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/**
 * "Let this PC control my phone": makes sure the control helper is switched on, asks Android
 * for permission to share the screen, then starts [ShareService].
 */
class ShareActivity : Activity() {
    private lateinit var col: LinearLayout
    private lateinit var pc: SavedPc
    private var askedProjection = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val saved = PcStore(this).get(intent.getStringExtra("pc") ?: "")
        if (saved == null) {
            finish()
            return
        }
        pc = saved
        col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(22), dp(36), dp(22), dp(32))
        }
        setContentView(ScrollView(this).apply {
            setBackgroundColor(0xFF101114.toInt())
            addView(col)
        })
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            // For the "Sharing with PC · Stop" notification.
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 2)
        }
    }

    override fun onResume() {
        super.onResume()
        if (!::pc.isInitialized) return
        render()
    }

    private fun text(t: String, size: Float = 15f, color: Int = 0xFFB8BEC8.toInt(), bold: Boolean = false) =
        TextView(this).apply {
            text = t
            textSize = size
            setTextColor(color)
            paint.isFakeBoldText = bold
            setLineSpacing(0f, 1.2f)
            setPadding(0, dp(6), 0, dp(6))
        }

    private fun render() {
        col.removeAllViews()
        val header = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        header.addView(pill("←") { finish() })
        header.addView(text("Control this phone from ${pc.name}", 22f, Color.WHITE, true).apply {
            setPadding(dp(14), 0, 0, 0)
        })
        col.addView(header)

        if (ShareService.running) {
            col.addView(text("Your phone is shared with ${pc.name}. A window showing this phone is open on the PC."))
            col.addView(pill("Stop sharing") {
                stopService(Intent(this, ShareService::class.java))
                finish()
            }, LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(16) })
            return
        }

        if (!ControlService.isEnabled(this)) {
            col.addView(text("Step 1 of 2: let EzRDP tap and type for the PC", 17f, Color.WHITE, true).apply {
                setPadding(0, dp(22), 0, dp(4))
            })
            col.addView(text(
                "Android needs you to switch this on once. Tap the button below, find EzRDP in the list " +
                    "(it may be under \"Installed apps\" or \"Downloaded apps\"), and turn it on.\n\n" +
                    "EzRDP only uses it while you're sharing with your PC."
            ))
            col.addView(pill("Open Accessibility settings") {
                startActivity(Intent(SystemSettings.ACTION_ACCESSIBILITY_SETTINGS))
            }, LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(12) })
            if (Build.VERSION.SDK_INT >= 33) {
                col.addView(text(
                    "If Android says it's a \"Restricted setting\": open App info for EzRDP, tap ⋮ (top right) → " +
                        "\"Allow restricted settings\", then try again.",
                    13f, 0xFF7C838E.toInt(),
                ).apply { setPadding(0, dp(18), 0, 0) })
                col.addView(pill("Open App info") {
                    startActivity(Intent(SystemSettings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
                }, LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(8) })
            }
            col.addView(text(
                "Without it, the PC can still see your screen but can't control it.",
                13f, 0xFF7C838E.toInt(),
            ).apply { setPadding(0, dp(18), 0, 0) })
            col.addView(pill("Just share the screen") { askProjection() },
                LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(8) })
            return
        }

        col.addView(text("Step 2 of 2: share your screen", 17f, Color.WHITE, true).apply {
            setPadding(0, dp(22), 0, dp(4))
        })
        col.addView(text(
            "Android will ask to share your screen. Choose \"Entire screen\" if asked, then Start.\n\n" +
                "On the PC: click to tap, drag to swipe, right-click = Back, middle-click = Home, " +
                "mouse wheel = scroll, and type to fill in text boxes."
        ))
        col.addView(pill("Start") { askProjection() }, LinearLayout.LayoutParams(-2, -2).apply { topMargin = dp(12) })
        if (!askedProjection) {
            askedProjection = true
            askProjection()
        }
    }

    private fun askProjection() {
        val mpm = getSystemService(MediaProjectionManager::class.java)
        startActivityForResult(mpm.createScreenCaptureIntent(), 1)
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != 1 || resultCode != RESULT_OK || data == null) return
        ShareService.start(
            this, pc.id,
            intent.getStringExtra("liveHost"), intent.getIntExtra("livePort", pc.port),
            resultCode, data,
        )
        finish()
    }
}
