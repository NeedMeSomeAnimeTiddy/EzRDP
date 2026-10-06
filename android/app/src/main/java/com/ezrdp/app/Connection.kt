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
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.PBEKeySpec
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
}

data class MonitorInfo(val index: Int, val primary: Boolean, val width: Int, val height: Int, val name: String)

class AuthException : IOException("Wrong password")
class VersionException : IOException("Version mismatch")

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

private object Keys {
    private val cache = HashMap<String, ByteArray>()

    /** PBKDF2 is slow on purpose, so remember the result for this password + PC. */
    @Synchronized
    fun master(password: String, salt: ByteArray, rounds: Int): ByteArray {
        val id = password + "|" + salt.joinToString("") { "%02x".format(it) } + "|" + rounds
        return cache.getOrPut(id) {
            val spec = PBEKeySpec(password.toCharArray(), salt, rounds, 256)
            SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256").generateSecret(spec).encoded
        }
    }

    fun derive(master: ByteArray, label: String, ns: ByteArray, nc: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(master, "HmacSHA256"))
        mac.update(label.toByteArray(Charsets.US_ASCII))
        mac.update(ns)
        mac.update(nc)
        return mac.doFinal()
    }
}

interface ConnectionListener {
    fun onMonitors(list: List<MonitorInfo>)
    fun onVideoConfig(monitor: Int, width: Int, height: Int)
    fun onVideoFrame(data: ByteArray, offset: Int, length: Int, keyframe: Boolean)
    fun onCursor(x: Float, y: Float, visible: Boolean)
    fun onAudioConfig(sampleRate: Int, channels: Int)
    fun onAudio(data: ByteArray, offset: Int, length: Int)
    fun onHomeAddress(ip: String)
    fun onDisconnected(error: Exception?)
}

/**
 * One TCP connection to the PC. Reading happens on its own thread; sends are queued
 * and written by a writer thread so the UI thread never touches the network.
 */
class Connection(
    private val host: String,
    private val port: Int,
    private val password: String,
    private val quality: Int,
    private val listener: ConnectionListener,
    private val connectTimeout: Int = 6000,
) {
    private lateinit var tx: Seal
    private lateinit var rx: Seal

    private val socket = Socket()
    private val queue = LinkedBlockingQueue<ByteArray>()
    @Volatile private var closed = false
    @Volatile private var userClosed = false

    /** Connects and logs in. Blocking: call from a background thread. */
    fun open() {
        socket.tcpNoDelay = true
        socket.receiveBufferSize = 1 shl 20
        socket.connect(InetSocketAddress(host, port), connectTimeout)
        socket.soTimeout = 10000 // the PC pings every 2s, so silence means the link is dead
        val input = DataInputStream(BufferedInputStream(socket.getInputStream(), 1 shl 16))
        val output = DataOutputStream(java.io.BufferedOutputStream(socket.getOutputStream(), 1 shl 16))

        // Login: prove we know the password without sending it, and agree on encryption keys.
        val hello = readPlain(input)
        if (hello.first != Msg.S_HELLO) throw IOException("Not an EzRDP server")
        val h = ByteBuffer.wrap(hello.second)
        if (h.get().toInt() != 2) throw VersionException()
        val salt = ByteArray(16).also { h.get(it) }
        val rounds = h.int
        val ns = ByteArray(16).also { h.get(it) }
        val nc = ByteArray(16).also { SecureRandom().nextBytes(it) }
        val master = Keys.master(password, salt, rounds)
        val proof = Keys.derive(master, "auth", ns, nc)
        val auth = nc + proof + byteArrayOf(quality.toByte())
        output.write(ByteBuffer.allocate(5 + auth.size).put(Msg.C_AUTH.toByte()).putInt(auth.size).put(auth).array())
        output.flush()
        val result = readPlain(input)
        if (result.first != Msg.S_AUTH_RESULT || result.second[0].toInt() != 1) throw AuthException()
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

    private fun readPlain(input: DataInputStream): Pair<Int, ByteArray> {
        val type = input.readUnsignedByte()
        val len = input.readInt()
        if (len < 0 || len > 4096) throw IOException("Not an EzRDP server")
        val body = ByteArray(len)
        input.readFully(body)
        return type to body
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
            Msg.S_INFO -> listener.onHomeAddress(String(msg, 1, msg.size - 1, Charsets.UTF_8))
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
}
