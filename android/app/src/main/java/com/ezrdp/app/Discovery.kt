package com.ezrdp.app

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException

/** A PC that answered on this Wi-Fi. */
data class FoundPc(val id: String, val name: String, val host: String, val port: Int, val paused: Boolean)

/** Finds EzRDP PCs on the same Wi-Fi by broadcasting "EZRDP?" (see server/src/discovery.rs). */
class Discovery(private val port: Int = 7270, private val onFound: (FoundPc) -> Unit) {
    @Volatile private var running = false
    private var socket: DatagramSocket? = null

    fun start() {
        if (running) return
        running = true
        Thread({ loop() }, "ezrdp-discovery").start()
    }

    fun stop() {
        running = false
        socket?.close()
    }

    private fun loop() {
        try {
            DatagramSocket().use { s ->
                socket = s
                s.broadcast = true
                s.soTimeout = 500
                val ask = "EZRDP?".toByteArray()
                val buf = ByteArray(512)
                var lastAsk = 0L
                while (running) {
                    val now = System.currentTimeMillis()
                    if (now - lastAsk > 2000) {
                        lastAsk = now
                        try {
                            s.send(DatagramPacket(ask, ask.size, InetAddress.getByName("255.255.255.255"), port))
                        } catch (_: Exception) {
                            // No network right now; keep trying.
                        }
                    }
                    val p = DatagramPacket(buf, buf.size)
                    try {
                        s.receive(p)
                    } catch (_: SocketTimeoutException) {
                        continue
                    }
                    parse(p)?.let(onFound)
                }
            }
        } catch (_: Exception) {
            // Socket closed by stop().
        }
    }

    private fun parse(p: DatagramPacket): FoundPc? {
        val d = p.data
        val n = p.length
        if (n < 6 + 1 + 16 + 2 + 1 || String(d, 0, 6, Charsets.US_ASCII) != "EZRDP!") return null
        if (d[6].toInt() != Msg.VERSION) return null
        val id = d.copyOfRange(7, 23).toHex()
        val port = ((d[23].toInt() and 0xff) shl 8) or (d[24].toInt() and 0xff)
        val paused = d[25].toInt() != 0
        val name = String(d, 26, n - 26, Charsets.UTF_8)
        return FoundPc(id, name, p.address.hostAddress ?: return null, port, paused)
    }
}

fun ByteArray.toHex() = joinToString("") { "%02x".format(it) }

fun String.fromHex(): ByteArray = ByteArray(length / 2) { substring(it * 2, it * 2 + 2).toInt(16).toByte() }
