package com.ezrdp.app

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.InputType
import android.view.View
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import java.net.Socket

/**
 * Lists paired PCs and PCs found on this Wi-Fi. Tap a paired PC to connect; tap a new one to
 * pair (both screens show the same code, you click Yes on the PC). No addresses or passwords.
 */
class MainActivity : Activity() {
    private val ui = Handler(Looper.getMainLooper())
    private lateinit var store: PcStore
    private lateinit var savedList: LinearLayout
    private lateinit var foundList: LinearLayout
    private lateinit var foundLabel: TextView
    private lateinit var emptyHint: TextView
    private val found = LinkedHashMap<String, FoundPc>()
    private val discovery = Discovery { pc -> ui.post { onFound(pc) } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        store = PcStore(this)

        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(44), dp(20), dp(24))
        }
        val header = LinearLayout(this).apply { gravity = android.view.Gravity.CENTER_VERTICAL }
        header.addView(TextView(this).apply {
            text = "EzRDP"
            textSize = 34f
            setTextColor(Color.WHITE)
            paint.isFakeBoldText = true
        }, LinearLayout.LayoutParams(0, -2, 1f))
        header.addView(pill("⚙  Settings") { startActivity(Intent(this, SettingsActivity::class.java)) })
        col.addView(header)
        col.addView(TextView(this).apply {
            text = "Control your PC from your phone."
            textSize = 15f
            setTextColor(0xFF9AA0AA.toInt())
            setPadding(0, dp(4), 0, dp(20))
        })

        col.addView(label("Your PCs"))
        savedList = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        col.addView(savedList)

        foundLabel = label("Found on this Wi-Fi")
        col.addView(foundLabel)
        foundList = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        col.addView(foundList)

        emptyHint = TextView(this).apply {
            text = "Looking for PCs on this Wi-Fi…\n\nMake sure EzRDP is running on your PC (its icon is " +
                "next to the clock) and this phone is on the same Wi-Fi. You only need to be home " +
                "for the first connection; after that it works from anywhere."
            textSize = 14f
            setTextColor(0xFF9AA0AA.toInt())
            setPadding(dp(2), dp(8), dp(2), dp(8))
        }
        col.addView(emptyHint)

        col.addView(TextView(this).apply {
            text = "Can't find your PC? Add it by address"
            textSize = 14f
            setTextColor(0xFF4F8CFF.toInt())
            setPadding(dp(2), dp(20), dp(2), dp(8))
            setOnClickListener { addByAddress() }
        })

        setContentView(ScrollView(this).apply {
            setBackgroundColor(0xFF101114.toInt())
            addView(col)
        })
    }

    override fun onResume() {
        super.onResume()
        found.clear()
        render()
        discovery.start()
    }

    override fun onPause() {
        super.onPause()
        discovery.stop()
    }

    private fun label(t: String) = TextView(this).apply {
        text = t
        textSize = 13f
        setTextColor(0xFF7C838E.toInt())
        isAllCaps = true
        letterSpacing = 0.08f
        setPadding(dp(2), dp(16), 0, dp(8))
    }

    private fun card(title: String, subtitle: String, onClick: () -> Unit, onLongClick: (() -> Unit)? = null): View {
        val c = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(14), dp(16), dp(14))
            background = rounded(0xFF1C1E23.toInt(), dp(12).toFloat(), dp(1), 0xFF2E3138.toInt())
            isClickable = true
            setOnClickListener { onClick() }
            if (onLongClick != null) setOnLongClickListener { onLongClick(); true }
        }
        c.addView(TextView(this).apply {
            text = title
            textSize = 18f
            setTextColor(Color.WHITE)
        })
        c.addView(TextView(this).apply {
            text = subtitle
            textSize = 13f
            setTextColor(0xFF9AA0AA.toInt())
            setPadding(0, dp(2), 0, 0)
        })
        c.layoutParams = LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(10) }
        return c
    }

    private fun onFound(pc: FoundPc) {
        val old = found[pc.id]
        found[pc.id] = pc
        if (old != pc) render()
    }

    private fun render() {
        val saved = store.all()
        savedList.removeAllViews()
        for (pc in saved) {
            val live = found[pc.id]
            val sub = when {
                live != null && live.version < Msg.VERSION -> "Older EzRDP on the PC · run the new EzRDP.exe there"
                live != null && live.version > Msg.VERSION -> "Newer EzRDP on the PC · update this app"
                live?.paused == true -> "Paused on the PC"
                live != null -> "On this Wi-Fi · tap to connect"
                pc.internet.isNotEmpty() || pc.manual.isNotEmpty() -> "Away from home · tap to connect"
                else -> "Not found on this Wi-Fi"
            }
            savedList.addView(card(pc.name, sub, { connect(pc, live) }, { options(pc, live) }))
        }
        if (saved.isNotEmpty()) {
            savedList.addView(TextView(this).apply {
                text = "Hold a PC for more, like letting it control this phone."
                textSize = 13f
                setTextColor(0xFF7C838E.toInt())
                setPadding(dp(2), dp(2), 0, dp(4))
            })
        }
        if (saved.isEmpty()) {
            savedList.addView(TextView(this).apply {
                text = "None yet. Tap your PC below to add it."
                textSize = 14f
                setTextColor(0xFF7C838E.toInt())
                setPadding(dp(2), 0, 0, dp(4))
            })
        }

        foundList.removeAllViews()
        val fresh = found.values.filter { f -> saved.none { it.id == f.id } }
        for (f in fresh) {
            val sub = when {
                f.version < Msg.VERSION -> "Older EzRDP on this PC · run the new EzRDP.exe there"
                f.version > Msg.VERSION -> "Newer EzRDP on this PC · update this app"
                f.paused -> "Paused on the PC"
                else -> "New · tap to add this PC"
            }
            foundList.addView(card(f.name, sub, { pair(f.host, f.port) }))
        }
        foundLabel.visibility = if (fresh.isEmpty()) View.GONE else View.VISIBLE
        emptyHint.visibility = if (found.isEmpty()) View.VISIBLE else View.GONE
    }

    private fun connect(pc: SavedPc, live: FoundPc?) {
        val i = Intent(this, SessionActivity::class.java).putExtra("pc", pc.id)
        if (live != null) i.putExtra("liveHost", live.host).putExtra("livePort", live.port)
        startActivity(i)
    }

    private fun options(pc: SavedPc, live: FoundPc?) {
        AlertDialog.Builder(this)
            .setTitle(pc.name)
            .setItems(arrayOf("Let this PC control my phone", "Set internet address…", "Forget this PC")) { _, which ->
                if (which == 0) {
                    val i = Intent(this, ShareActivity::class.java).putExtra("pc", pc.id)
                    if (live != null) i.putExtra("liveHost", live.host).putExtra("livePort", live.port)
                    startActivity(i)
                } else if (which == 1) setManualAddress(pc) else {
                    AlertDialog.Builder(this)
                        .setMessage("Forget ${pc.name}? You'll need to add it again (at home) to connect.")
                        .setPositiveButton("Forget") { _, _ ->
                            store.remove(pc.id)
                            render()
                        }
                        .setNegativeButton("Cancel", null)
                        .show()
                }
            }
            .show()
    }

    private fun input(hint: String, value: String) = EditText(this).apply {
        setText(value)
        this.hint = hint
        inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
        setSingleLine()
    }

    /** Optional: for people with a DDNS name or a changing internet address. */
    private fun setManualAddress(pc: SavedPc) {
        val field = input("e.g. myhome.ddns.net", pc.manual)
        AlertDialog.Builder(this)
            .setTitle("Internet address")
            .setMessage("Usually not needed: EzRDP learns it automatically. Use this if you have a fixed name for your home internet (DDNS).")
            .setView(field)
            .setPositiveButton("Save") { _, _ ->
                pc.manual = field.text.toString().trim()
                store.save(pc)
                render()
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun addByAddress() {
        val field = input("PC's home address, e.g. 192.168.1.20", "")
        AlertDialog.Builder(this)
            .setTitle("Add PC by address")
            .setMessage("Use the Home address from the EzRDP tray menu → Connection info. Your phone must be on the same Wi-Fi.")
            .setView(field)
            .setPositiveButton("Add") { _, _ ->
                val text = field.text.toString().trim()
                if (text.isNotEmpty()) {
                    val host = text.substringBefore(':')
                    val port = text.substringAfter(':', "7270").toIntOrNull() ?: 7270
                    pair(host, port)
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun phoneName(): String {
        val custom = Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME)
        return if (!custom.isNullOrBlank()) custom else "${Build.MANUFACTURER} ${Build.MODEL}".trim()
    }

    private fun pair(host: String, port: Int) {
        var socket: Socket? = null
        var cancelled = false
        val dialog = AlertDialog.Builder(this)
            .setTitle("Adding PC")
            .setMessage("Connecting…")
            .setNegativeButton("Cancel") { _, _ ->
                cancelled = true
                Thread { try { socket?.close() } catch (_: Exception) {} }.start()
            }
            .setCancelable(false)
            .show()
        Thread {
            try {
                val r = Pairing.pair(host, port, phoneName(), { socket = it }) { code ->
                    ui.post {
                        dialog.setMessage(
                            "Your PC is asking whether to trust this phone.\n\n" +
                                "Check it shows this code, then click Yes on the PC:\n\n$code",
                        )
                    }
                }
                val pc = SavedPc(r.pcId.toHex(), r.pcName, r.deviceId, r.key, home = host, port = port)
                store.save(pc)
                ui.post {
                    dialog.dismiss()
                    Toast.makeText(this, "${r.pcName} added", Toast.LENGTH_SHORT).show()
                    render()
                    connect(pc, FoundPc(pc.id, pc.name, host, port, false))
                }
            } catch (e: Exception) {
                ui.post {
                    dialog.dismiss()
                    if (!cancelled) showPairError(e, host)
                }
            }
        }.start()
    }

    private fun showPairError(e: Exception, host: String) {
        val msg = when {
            e is Pairing.Refused && e.reason == Msg.RESULT_DENIED -> "The PC didn't allow this phone."
            e is Pairing.Refused && e.reason == Msg.RESULT_NOT_LOCAL ->
                "Adding a PC only works when your phone is on the same Wi-Fi as the PC."
            e is Pairing.Refused && e.reason == Msg.RESULT_BUSY ->
                "The PC is busy adding another phone. Try again in a moment."
            e is Pairing.Refused && e.reason == Msg.RESULT_PAUSED ->
                "Connections are paused on the PC. Right-click the EzRDP icon on the PC and choose Resume."
            e is VersionException -> e.advice("The PC")
            e is java.net.SocketTimeoutException -> "No answer on the PC in time. Try again."
            else -> "Couldn't reach the PC at $host. Is EzRDP running on it?"
        }
        AlertDialog.Builder(this).setTitle("Couldn't add PC").setMessage(msg).setPositiveButton("OK", null).show()
    }
}
