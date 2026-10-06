package com.ezrdp.app

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.util.Log
import android.view.Surface

/**
 * Hardware H.264 decoding straight onto a Surface, tuned for lowest delay:
 * every decoded frame is shown immediately, nothing is buffered.
 */
class VideoDecoder(private val requestKeyframe: () -> Unit) {
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var width = 0
    private var height = 0
    private var outputThread: Thread? = null
    private var lastKeyRequest = 0L

    @Synchronized
    fun setSurface(s: Surface?) {
        if (s == surface) return
        release()
        surface = s
        if (s != null) askForKeyframe()
    }

    @Synchronized
    fun configure(w: Int, h: Int) {
        if (w == width && h == height && codec != null) return
        release()
        width = w
        height = h
    }

    @Synchronized
    fun decode(data: ByteArray, offset: Int, length: Int, keyframe: Boolean) {
        val s = surface ?: return
        if (codec == null) {
            if (!keyframe || width == 0) {
                askForKeyframe()
                return
            }
            codec = start(s, data, offset, length) ?: return
        }
        val c = codec ?: return
        try {
            val index = c.dequeueInputBuffer(30_000)
            if (index < 0) {
                // Decoder is backed up; skipping breaks the picture, so get a fresh keyframe.
                askForKeyframe()
                return
            }
            val buf = c.getInputBuffer(index) ?: return
            buf.clear()
            buf.put(data, offset, length)
            c.queueInputBuffer(index, 0, length, System.nanoTime() / 1000, 0)
        } catch (e: Exception) {
            Log.w(TAG, "decode failed, restarting decoder", e)
            release()
            askForKeyframe()
        }
    }

    private fun askForKeyframe() {
        val now = System.currentTimeMillis()
        if (now - lastKeyRequest > 500) {
            lastKeyRequest = now
            requestKeyframe()
        }
    }

    private fun start(s: Surface, data: ByteArray, offset: Int, length: Int): MediaCodec? {
        val nals = splitNals(data, offset, length)
        val sps = nals.firstOrNull { it.type == 7 }
        val pps = nals.firstOrNull { it.type == 8 }
        if (sps == null || pps == null) {
            askForKeyframe()
            return null
        }
        fun format(lowLatency: Boolean): MediaFormat {
            val f = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height)
            f.setByteBuffer("csd-0", java.nio.ByteBuffer.wrap(data, sps.start, sps.end - sps.start).slice())
            f.setByteBuffer("csd-1", java.nio.ByteBuffer.wrap(data, pps.start, pps.end - pps.start).slice())
            f.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, width * height)
            if (lowLatency) {
                f.setInteger(MediaFormat.KEY_PRIORITY, 0)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    f.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                }
                // Vendor low-latency switches (ignored by decoders that don't know them).
                f.setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
                f.setInteger("vendor.qti-ext-dec-picture-order.enable", 1)
                f.setInteger("vendor.rtc-ext-dec-low-latency.enable", 1)
                f.setInteger("vendor.low-latency.enable", 1)
            }
            return f
        }
        // Some phones refuse the low-latency settings; fall back to plain decoding.
        for (lowLatency in listOf(true, false)) {
            var c: MediaCodec? = null
            try {
                c = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
                c.configure(format(lowLatency), s, null, 0)
                c.start()
                val t = Thread({ drain(c) }, "ezrdp-video-out")
                t.start()
                outputThread = t
                return c
            } catch (e: Exception) {
                Log.e(TAG, "could not start decoder (lowLatency=$lowLatency)", e)
                c?.release()
            }
        }
        return null
    }

    private fun drain(c: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        try {
            while (!Thread.currentThread().isInterrupted) {
                val index = c.dequeueOutputBuffer(info, 20_000)
                if (index >= 0) c.releaseOutputBuffer(index, true)
            }
        } catch (_: Exception) {
            // Codec was released.
        }
    }

    @Synchronized
    fun release() {
        outputThread?.interrupt()
        outputThread = null
        codec?.let {
            try { it.stop() } catch (_: Exception) {}
            try { it.release() } catch (_: Exception) {}
        }
        codec = null
    }

    private class Nal(val start: Int, val end: Int, val type: Int)

    /** Splits Annex-B data into NAL units (start codes included). */
    private fun splitNals(d: ByteArray, off: Int, len: Int): List<Nal> {
        val starts = ArrayList<Int>()
        val end = off + len
        var i = off
        while (i + 3 < end) {
            if (d[i].toInt() == 0 && d[i + 1].toInt() == 0 &&
                (d[i + 2].toInt() == 1 || (d[i + 2].toInt() == 0 && d[i + 3].toInt() == 1))
            ) {
                starts.add(i)
                i += if (d[i + 2].toInt() == 1) 3 else 4
            } else {
                i++
            }
        }
        return starts.mapIndexed { n, s ->
            val codeLen = if (d[s + 2].toInt() == 1) 3 else 4
            val e = if (n + 1 < starts.size) starts[n + 1] else end
            Nal(s, e, d[s + codeLen].toInt() and 0x1f)
        }
    }

    companion object {
        private const val TAG = "EzRDP"
    }
}
