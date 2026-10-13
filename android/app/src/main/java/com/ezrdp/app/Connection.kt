package com.ezrdp.app

import java.io.BufferedInputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.net.InetSocketAddress
import java.net.Socket
import java.nio.ByteBuffer
import java.util.concurrent.LinkedBlockingQueue
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** Message types. Must match server/src/protocol.rs. */
object Msg {
    const val S_HELLO = 0x01
    const val S_AUTH_RESULT = 0x02
    const val S_MONITORS = 0x03
    const val S_VIDEO_CONFIG = 0x04
    const val S_VIDEO_FRAME = 0x05
    const val S_CURSOR = 0x06
    const val S_AUDIO_CONFIG = 0x07
    const val S_AUDIO = 0x08
    const val S_PING = 0x09
    const val S_INFO = 0x0A
    const val S_PAIR_KEY = 0x0B
    const val S_PAIR_RESULT = 0x0C
    const val S_BYE = 0x0D
    /** Text to show the person (e.g. why the mouse isn't working). */
    const val S_NOTICE = 0x0E

    const val BYE_ENDED = 0
    const val BYE_PAUSED = 1
    const val BYE_REMOVED = 2

    const val C_AUTH = 0x81
    const val C_MOUSE_ABS = 0x82
    const val C_MOUSE_REL = 0x83
    const val C_MOUSE_BUTTON = 0x84
    const val C_SCROLL = 0x85
    const val C_TEXT = 0x86
    const val C_KEY = 0x87
    const val C_SELECT_MONITOR = 0x88
    const val C_FRAME_ACK = 0x89
    const val C_SET_AUDIO = 0x8A
    const val C_KEYFRAME = 0x8B
    const val C_SET_QUALITY = 0x8C
    const val C_PING = 0x8D
    const val C_PAIR_START = 0x8E
    const val C_PAIR_KEY = 0x8F

    // Sharing this phone's screen with the PC (role 1).
    const val P_SCREEN_CONFIG = 0x90
    const val P_SCREEN_FRAME = 0x91
    const val S_TOUCH = 0x20
    const val S_NAV = 0x21
    const val S_TYPE = 0x22
    const val S_SWIPE = 0x23
    const val S_FRAME_ACK = 0x24
    const val S_KEYFRAME = 0x25

    const val NAV_BACK = 0
    const val NAV_HOME = 1
    const val NAV_RECENTS = 2
    const val NAV_NOTIFICATIONS = 3
    const val NAV_BACKSPACE = 4
    const val NAV_ENTER = 5

    const val ROLE_CONTROL_PC = 0
    const val ROLE_SHARE_PHONE = 1

    const val RESULT_DENIED = 0
    const val RESULT_OK = 1
    const val RESULT_PAUSED = 2
    const val RESULT_NOT_LOCAL = 3
    const val RESULT_BUSY = 4

    const val VERSION = 4
}

data class MonitorInfo(val index: Int, val primary: Boolean, val width: Int, val height: Int, val name: String)

/** The PC no longer trusts this phone (it was removed on the PC). */
class NotTrustedException : IOException("Not trusted")
class VersionException(val pcVersion: Int) : IOException("Version mismatch") {
    /** What to tell the person, depending on which side is out of date. */
    fun advice(pcName: String): String = if (pcVersion < Msg.VERSION) {
        "$pcName is running an older EzRDP (version $pcVersion; this phone has version ${Msg.VERSION}).\n\n" +
            "On the PC, run the new EzRDP.exe. It closes the old one by itself. " +
            "If you put the new file in a different folder, delete the old one so Windows doesn't start it again."
    } else {
        "$pcName has a newer EzRDP (version $pcVersion; this phone has version ${Msg.VERSION}).\n\n" +
            "Install the new EzRDP app on this phone."
    }
}
class PausedException : IOException("Paused")
/** Something answered at this address, but it's a different PC. */
class WrongPcException : IOException("Different PC")

/** AES-GCM for one direction; the message counter is the nonce (must match server/src/crypto.rs). */
private class Seal(key: ByteArray) {
    private val key = SecretKeySpec(key, 0, 16, "AES")
    private var counter = 0L
    private val cipher = Cipher.getInstance("AES/GCM/NoPadding")

    private fun nonce(): GCMParameterSpec {
        val n = ByteBuffer.allocate(12).putInt(0).putLong(counter++).array()
        return GCMParameterSpec(128, n)
    }

    fun seal(plain: ByteArray): ByteArray {
        cipher.init(Cipher.ENCRYPT_MODE, key, nonce())
        return cipher.doFinal(plain)
    }

    fun open(sealed: ByteArray): ByteArray {
        cipher.init(Cipher.DECRYPT_MODE, key, nonce())
        return cipher.doFinal(sealed)
    }
}

internal object Keys {
    fun derive(key: ByteArray, label: String, ns: ByteArray, nc: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        mac.update(label.toByteArray(Charsets.US_ASCII))
        mac.update(ns)
        mac.update(nc)
        return mac.doFinal()
    }
}

/** What the PC says first: who it is. */
internal class Hello(val pcId: ByteArray, val nonce: ByteArray, val paused: Boolean, val name: String)

internal fun readPlain(input: DataInputStream): Pair<Int, ByteArray> {
    val type = input.readUnsignedByte()
    val len = input.readInt()
    if (len < 0 || len > 4096) throw IOException("Not an EzRDP PC")
    val body = ByteArray(len)
    input.readFully(body)
    return type to body
}

internal fun writePlain(output: java.io.OutputStream, type: Int, payload: ByteArray) {
    output.write(ByteBuffer.allocate(5 + payload.size).put(type.toByte()).putInt(payload.size).put(payload).array())
    output.flush()
}

internal fun readHello(input: DataInputStream): Hello {
    val (type, body) = readPlain(input)
    if (type != Msg.S_HELLO || body.isEmpty()) throw IOException("Not an EzRDP PC")
    if (body[0].toInt() != Msg.VERSION) throw VersionException(body[0].toInt() and 0xff)
    val b = ByteBuffer.wrap(body, 1, body.size - 1)
    val id = ByteArray(16).also { b.get(it) }
    val nonce = ByteArray(16).also { b.get(it) }
    val paused = b.get().toInt() != 0
    val name = String(body, b.position(), b.remaining(), Charsets.UTF_8)
    return Hello(id, nonce, paused, name)
}

interface ConnectionListener {
    fun onMonitors(list: List<MonitorInfo>) {}
    fun onVideoConfig(monitor: Int, width: Int, height: Int) {}
    fun onVideoFrame(data: ByteArray, offset: Int, length: Int, keyframe: Boolean) {}
    fun onCursor(x: Float, y: Float, visible: Boolean) {}
    fun onAudioConfig(sampleRate: Int, channels: Int) {}
    fun onAudio(data: ByteArray, offset: Int, length: Int) {}
    /** The PC's current home address, internet address and port, to remember for next time. */
    fun onAddresses(home: String, internet: String, port: Int) {}
    fun onDisconnected(error: Exception?)
    /** The PC ended the session on purpose (BYE_*): don't reconnect. */
    fun onBye(reason: Int) {}
    /** Something the PC wants the person to know. */
    fun onNotice(text: String) {}

    // While sharing this phone's screen, the PC sends these.
    /** action 0 = finger down, 1 = move, 2 = up; x/y are 0..1 of the phone screen. */
    fun onTouch(action: Int, x: Float, y: Float) {}
    fun onNav(what: Int) {}
    fun onType(text: String) {}
    fun onSwipe(x: Float, y: Float, dx: Float, dy: Float) {}
    fun onFrameAck(id: Int) {}
    fun onKeyframeRequest() {}
}

/** Extra details sent when logging in. */
class LoginOptions(
    val quality: Int = 1,
    val role: Int = Msg.ROLE_CONTROL_PC,
    /** Biggest picture this phone can decode (0 = unknown). */
    val maxWidth: Int = 0,
    val maxHeight: Int = 0,
    /** Let the PC use its graphics card to encode (faster). */
    val gpuEncoder: Boolean = true,
)

/**
 * One TCP connection to the PC. Reading happens on its own thread; sends are queued
 * and written by a writer thread so the UI thread never touches the network.
 */
class Connection(
    private val host: String,
    private val port: Int,
    /** The PC we expect at this address (null = any). */
    private val pcId: ByteArray?,
    private val deviceId: ByteArray,
    private val deviceKey: ByteArray,
    private val options: LoginOptions,
    private val listener: ConnectionListener,
    private val connectTimeout: Int = 6000,
) {
    private lateinit var tx: Seal
    private lateinit var rx: Seal

    private val socket = Socket()
    private val queue = LinkedBlockingQueue<ByteArray>()
    /** The connection has ended (either side). */
    @Volatile var closed = false
        private set
    @Volatile private var userClosed = false

    /** Connects and logs in. Blocking: call from a background thread. */
    fun open() {
        socket.tcpNoDelay = true
        socket.receiveBufferSize = 1 shl 20
        socket.connect(InetSocketAddress(host, port), connectTimeout)
        socket.soTimeout = 10000 // the PC pings every 2s, so silence means the link is dead
        val input = DataInputStream(BufferedInputStream(socket.getInputStream(), 1 shl 16))
        val output = DataOutputStream(java.io.BufferedOutputStream(socket.getOutputStream(), 1 shl 16))

        // Login: prove we're a trusted phone, and agree on encryption keys.
        val hello = readHello(input)
        if (pcId != null && !hello.pcId.contentEquals(pcId)) throw WrongPcException()
        if (hello.paused) throw PausedException()
        val ns = hello.nonce
        val nc = ByteArray(16).also { SecureRandom().nextBytes(it) }
        val proof = Keys.derive(deviceKey, "auth", ns, nc)
        val extra = ByteBuffer.allocate(7)
            .put(options.quality.toByte())
            .put(options.role.toByte())
            .putShort(options.maxWidth.coerceIn(0, 65535).toShort())
            .putShort(options.maxHeight.coerceIn(0, 65535).toShort())
            .put(if (options.gpuEncoder) 1 else 0)
            .array()
        writePlain(output, Msg.C_AUTH, deviceId + nc + proof + extra)
        val result = readPlain(input)
        if (result.first != Msg.S_AUTH_RESULT) throw IOException("Not an EzRDP PC")
        when (result.second[0].toInt()) {
            Msg.RESULT_OK -> {}
            Msg.RESULT_PAUSED -> throw PausedException()
            else -> throw NotTrustedException()
        }
        val master = deviceKey
        tx = Seal(Keys.derive(master, "c2s", ns, nc))
        rx = Seal(Keys.derive(master, "s2c", ns, nc))

        Thread({ writeLoop(output) }, "ezrdp-writer").start()
        Thread({ readLoop(input) }, "ezrdp-reader").start()
        Thread({
            while (!closed) {
                send(Msg.C_PING)
                try { Thread.sleep(3000) } catch (_: InterruptedException) { break }
            }
        }, "ezrdp-ping").start()
    }

    /** Hang up on purpose (no reconnect). */
    fun close() {
        userClosed = true
        shutdown()
    }

    private fun shutdown() {
        if (closed) return
        closed = true
        queue.offer(ByteArray(0))
        try { socket.close() } catch (_: IOException) {}
    }

    /** Reads one encrypted message; returns the plaintext (type byte first). */
    private fun readSealed(input: DataInputStream): ByteArray {
        val len = input.readInt()
        if (len < 17 || len > 64 shl 20) throw IOException("Bad message")
        val body = ByteArray(len)
        input.readFully(body)
        return rx.open(body)
    }

    private fun readLoop(input: DataInputStream) {
        var error: Exception? = null
        try {
            while (!closed) {
                handle(readSealed(input))
            }
        } catch (e: Exception) {
            error = e
        }
        shutdown()
        listener.onDisconnected(if (userClosed) null else (error ?: IOException("Connection closed")))
    }

    private fun handle(msg: ByteArray) {
        val type = msg[0].toInt() and 0xff
        val b = ByteBuffer.wrap(msg, 1, msg.size - 1)
        when (type) {
            Msg.S_MONITORS -> {
                val count = b.get().toInt() and 0xff
                val list = ArrayList<MonitorInfo>()
                repeat(count) {
                    val index = b.get().toInt() and 0xff
                    val primary = b.get().toInt() != 0
                    val w = b.short.toInt() and 0xffff
                    val h = b.short.toInt() and 0xffff
                    val nameLen = b.short.toInt() and 0xffff
                    val name = String(msg, b.position(), nameLen, Charsets.UTF_8)
                    b.position(b.position() + nameLen)
                    list.add(MonitorInfo(index, primary, w, h, name))
                }
                listener.onMonitors(list)
            }
            Msg.S_VIDEO_CONFIG -> {
                val mon = b.get().toInt() and 0xff
                val w = b.short.toInt() and 0xffff
                val h = b.short.toInt() and 0xffff
                listener.onVideoConfig(mon, w, h)
            }
            Msg.S_VIDEO_FRAME -> {
                val id = b.int
                val key = b.get().toInt() != 0
                // Tell the PC this frame arrived so it can send the next one.
                send(Msg.C_FRAME_ACK, ByteBuffer.allocate(4).putInt(id).array())
                listener.onVideoFrame(msg, 6, msg.size - 6, key)
            }
            Msg.S_CURSOR -> {
                val x = b.float
                val y = b.float
                listener.onCursor(x, y, b.get().toInt() != 0)
            }
            Msg.S_AUDIO_CONFIG -> listener.onAudioConfig(b.int, b.get().toInt() and 0xff)
            Msg.S_AUDIO -> listener.onAudio(msg, 1, msg.size - 1)
            Msg.S_INFO -> {
                val homeLen = b.get().toInt() and 0xff
                val home = String(msg, b.position(), homeLen, Charsets.UTF_8)
                b.position(b.position() + homeLen)
                val netLen = b.get().toInt() and 0xff
                val net = String(msg, b.position(), netLen, Charsets.UTF_8)
                b.position(b.position() + netLen)
                listener.onAddresses(home, net, b.short.toInt() and 0xffff)
            }
            Msg.S_BYE -> listener.onBye(b.get().toInt())
            Msg.S_NOTICE -> listener.onNotice(String(msg, 1, msg.size - 1, Charsets.UTF_8))
            Msg.S_TOUCH -> {
                val action = b.get().toInt()
                listener.onTouch(action, b.float, b.float)
            }
            Msg.S_NAV -> listener.onNav(b.get().toInt())
            Msg.S_TYPE -> listener.onType(String(msg, 1, msg.size - 1, Charsets.UTF_8))
            Msg.S_SWIPE -> listener.onSwipe(b.float, b.float, b.float, b.float)
            Msg.S_FRAME_ACK -> listener.onFrameAck(b.int)
            Msg.S_KEYFRAME -> listener.onKeyframeRequest()
        }
    }

    private fun writeLoop(output: DataOutputStream) {
        try {
            while (!closed) {
                val msg = queue.take()
                if (msg.isEmpty()) break
                writeSealed(output, msg)
                // Batch whatever else is already waiting into one flush.
                while (true) {
                    val more = queue.poll() ?: break
                    if (more.isEmpty()) return
                    writeSealed(output, more)
                }
                output.flush()
            }
        } catch (_: Exception) {
            shutdown()
        }
    }

    private fun writeSealed(output: DataOutputStream, plain: ByteArray) {
        val sealed = tx.seal(plain)
        output.writeInt(sealed.size)
        output.write(sealed)
    }

    fun send(type: Int, payload: ByteArray = ByteArray(0)) {
        if (!closed) queue.offer(byteArrayOf(type.toByte()) + payload)
    }

    // --- Convenience senders ---

    fun mouseAbs(x: Float, y: Float) =
        send(Msg.C_MOUSE_ABS, ByteBuffer.allocate(8).putFloat(x).putFloat(y).array())

    fun mouseRel(dx: Float, dy: Float) =
        send(Msg.C_MOUSE_REL, ByteBuffer.allocate(8).putFloat(dx).putFloat(dy).array())

    fun button(button: Int, down: Boolean) =
        send(Msg.C_MOUSE_BUTTON, byteArrayOf(button.toByte(), if (down) 1 else 0))

    fun click(button: Int) {
        button(button, true)
        button(button, false)
    }

    fun scroll(dy: Int, dx: Int) = send(
        Msg.C_SCROLL,
        ByteBuffer.allocate(4).putShort(dy.coerceIn(-32000, 32000).toShort())
            .putShort(dx.coerceIn(-32000, 32000).toShort()).array(),
    )

    fun text(s: String) = send(Msg.C_TEXT, s.toByteArray(Charsets.UTF_8))

    fun key(vk: Int, down: Boolean) =
        send(Msg.C_KEY, ByteBuffer.allocate(3).putShort(vk.toShort()).put(if (down) 1 else 0).array())

    fun tapKey(vk: Int) {
        key(vk, true)
        key(vk, false)
    }

    fun selectMonitor(i: Int) = send(Msg.C_SELECT_MONITOR, byteArrayOf(i.toByte()))
    fun setAudio(on: Boolean) = send(Msg.C_SET_AUDIO, byteArrayOf(if (on) 1 else 0))
    fun requestKeyframe() = send(Msg.C_KEYFRAME)
    fun setQuality(q: Int) = send(Msg.C_SET_QUALITY, byteArrayOf(q.toByte()))

    // --- Sharing this phone's screen ---

    fun sendScreenConfig(w: Int, h: Int) =
        send(Msg.P_SCREEN_CONFIG, ByteBuffer.allocate(4).putShort(w.toShort()).putShort(h.toShort()).array())

    fun sendScreenFrame(id: Int, keyframe: Boolean, data: ByteArray, offset: Int, length: Int) {
        val p = ByteArray(5 + length)
        ByteBuffer.wrap(p).putInt(id).put(if (keyframe) 1 else 0)
        System.arraycopy(data, offset, p, 5, length)
        send(Msg.P_SCREEN_FRAME, p)
    }

    /** Messages waiting to go out (used to avoid piling up video on a slow link). */
    fun queued(): Int = queue.size
}
