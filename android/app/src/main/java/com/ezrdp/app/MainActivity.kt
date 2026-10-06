package com.ezrdp.app

import android.app.Activity
import android.content.Intent
import android.graphics.Color
import android.os.Bundle
import android.text.InputType
import android.view.Gravity
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.TextView

/** The connect screen: PC address, password, quality. Everything is remembered. */
class MainActivity : Activity() {
    private lateinit var hostField: EditText
    private lateinit var portField: EditText
    private lateinit var passField: EditText
    private lateinit var qualityGroup: RadioGroup

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val prefs = getSharedPreferences("ezrdp", MODE_PRIVATE)

        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(48), dp(24), dp(24))
        }

        col.addView(TextView(this).apply {
            text = "EzRDP"
            textSize = 34f
            setTextColor(Color.WHITE)
            paint.isFakeBoldText = true
        })
        col.addView(TextView(this).apply {
            text = "Control your PC from your phone."
            textSize = 15f
            setTextColor(0xFF9AA0AA.toInt())
            setPadding(0, dp(4), 0, dp(28))
        })

        fun label(t: String) = TextView(this).apply {
            text = t
            textSize = 14f
            setTextColor(0xFFB8BEC8.toInt())
            setPadding(dp(2), dp(14), 0, dp(6))
        }

        fun field(hint: String, value: String, type: Int) = EditText(this).apply {
            setText(value)
            this.hint = hint
            inputType = type
            setSingleLine()
            setTextColor(Color.WHITE)
            setHintTextColor(0xFF5C626C.toInt())
            textSize = 17f
            setPadding(dp(14), dp(12), dp(14), dp(12))
            background = rounded(0xFF1C1E23.toInt(), dp(10).toFloat(), dp(1), 0xFF2E3138.toInt())
        }

        col.addView(label("PC address"))
        hostField = field("the Address shown on your PC", prefs.getString("host", "") ?: "", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        col.addView(hostField)

        col.addView(label("Port"))
        portField = field("7270", prefs.getInt("port", 7270).toString(), InputType.TYPE_CLASS_NUMBER)
        col.addView(portField)

        col.addView(label("Password"))
        passField = field("shown in the EzRDP window on your PC", prefs.getString("password", "") ?: "", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD)
        col.addView(passField)

        col.addView(label("Quality"))
        qualityGroup = RadioGroup(this).apply { orientation = RadioGroup.VERTICAL }
        listOf("High — sharpest, for home Wi-Fi", "Balanced", "Data saver — for mobile data").forEachIndexed { i, t ->
            qualityGroup.addView(RadioButton(this).apply {
                id = 100 + i
                text = t
                setTextColor(Color.WHITE)
                textSize = 15f
            })
        }
        qualityGroup.check(100 + prefs.getInt("quality", 1))
        col.addView(qualityGroup)

        val connect = TextView(this).apply {
            text = "Connect"
            textSize = 18f
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
            setPadding(0, dp(16), 0, dp(16))
            background = rounded(0xFF4F8CFF.toInt(), dp(12).toFloat())
            setOnClickListener { connect() }
        }
        col.addView(connect, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(28) })

        col.addView(TextView(this).apply {
            text = "Use the Address shown in the EzRDP window on your PC. It works on home Wi-Fi and " +
                "on mobile data, and everything is encrypted with your password."
            textSize = 13f
            setTextColor(0xFF7C838E.toInt())
            setPadding(dp(2), dp(20), dp(2), 0)
        })

        setContentView(ScrollView(this).apply {
            setBackgroundColor(0xFF101114.toInt())
            addView(col)
        })
    }

    private fun connect() {
        val host = hostField.text.toString().trim()
        val port = portField.text.toString().trim().toIntOrNull() ?: 7270
        val password = passField.text.toString().trim()
        val quality = (qualityGroup.checkedRadioButtonId - 100).coerceIn(0, 2)
        if (host.isEmpty()) {
            hostField.error = "Enter your PC's address"
            return
        }
        if (password.isEmpty()) {
            passField.error = "Enter the password from the PC"
            return
        }
        getSharedPreferences("ezrdp", MODE_PRIVATE).edit()
            .putString("host", host)
            .putInt("port", port)
            .putString("password", password)
            .putInt("quality", quality)
            .apply()
        startActivity(
            Intent(this, SessionActivity::class.java)
                .putExtra("host", host)
                .putExtra("port", port)
                .putExtra("password", password)
                .putExtra("quality", quality),
        )
    }
}
