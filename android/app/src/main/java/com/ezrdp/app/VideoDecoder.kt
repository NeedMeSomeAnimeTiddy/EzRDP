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
    /** Biggest frame the input buffers were sized for; bigger frames restart the decoder. */
    private var inputSize = 0
    private var startedAt = 0L
    private var fed = 0
    @Volatile private var shown = 0
    /** Frames fed in since the last picture came out; many in a row means the decoder stalled. */
    @Volatile private var sinceOutput = 0
    private var stuckReported = false
    /** The decoder can't go on without a full picture (keyframe) and hasn't had one yet. */
    @Volatile var needsKeyframe = true
        private set

    /** Called (on the decoder's thread) when the first picture of a stream is on screen. */
    var onPicture: () -> Unit = {}
    /** Called when the decoder takes video but never shows anything (e.g. too big for this phone). */
    var onStuck: () -> Unit = {}

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
        stuckReported = false
    }

    @Synchronized
    fun decode(data: ByteArray, offset: Int, length: Int, keyframe: Boolean, retry: Boolean = true) {
        val s = surface ?: return askForKeyframe()
        if (codec == null) {
            if (!keyframe || width == 0) {
                askForKeyframe()
                return
            }
            codec = start(s, data, offset, length) ?: return
        }
        val c = codec ?: return
        if (shown > 0 && sinceOutput > 45) {
            // Pictures go in but none come out any more: start the decoder again.
            Log.w(TAG, "decoder stopped giving pictures; restarting it")
            release()
            askForKeyframe()
            return
        }
        if (length > inputSize && retry) {
            // Frame bigger than the decoder's buffers (sharp keyframes can be): restart bigger.
            Log.i(TAG, "frame of $length bytes; restarting decoder with bigger buffers")
            release()
            if (keyframe) {
                inputSize = length * 2
                codec = start(s, data, offset, length) ?: return
                decode(data, offset, length, keyframe, retry = false)
            } else {
                inputSize = length * 2
                askForKeyframe()
            }
            return
        }
        if (fed > 20 && shown == 0 && !stuckReported &&
            System.currentTimeMillis() - startedAt > 2500
        ) {
            stuckReported = true
            Log.w(TAG, "decoder took $fed frames but showed nothing")
            onStuck()
        }
        try {
            val index = c.dequeueInputBuffer(30_000)
            if (index < 0) {
                // Decoder is backed up; skipping breaks the picture, so get a fresh keyframe.
                askForKeyframe()
                return
            }
            val buf = c.getInputBuffer(index) ?: return
            if (buf.capacity() < length) {
                c.queueInputBuffer(index, 0, 0, 0, 0)
                if (retry) {
                    inputSize = buf.capacity()
                    decode(data, offset, length, keyframe)
                } else {
                    askForKeyframe()
                }
                return
            }
            buf.clear()
            buf.put(data, offset, length)
            c.queueInputBuffer(index, 0, length, System.nanoTime() / 1000, 0)
            fed++
            sinceOutput++
            if (keyframe) needsKeyframe = false
        } catch (e: Exception) {
            Log.w(TAG, "decode failed, restarting decoder", e)
            release()
            askForKeyframe()
        }
    }

    private fun askForKeyframe() {
        needsKeyframe = true
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
            f.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, inputSize)
            if (lowLatency) {
                f.setInteger(MediaFormat.KEY_PRIORITY, 0)
                // Run the decoder at full speed instead of a power-saving pace.
                f.setInteger(MediaFormat.KEY_OPERATING_RATE, Short.MAX_VALUE.toInt())
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
        inputSize = maxOf(inputSize, width * height * 2, length * 2, 1 shl 20)
        fed = 0
        shown = 0
        sinceOutput = 0
        startedAt = System.currentTimeMillis()
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
                if (index >= 0) {
                    c.releaseOutputBuffer(index, true)
                    sinceOutput = 0
                    if (shown++ == 0) onPicture()
                }
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
        needsKeyframe = true
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

        /**
         * Biggest landscape picture this phone's H.264 decoder can play at 60 fps (0,0 if unknown).
         * The PC never sends anything bigger.
         */
        fun maxSize(): Pair<Int, Int> {
            try {
                val list = android.media.MediaCodecList(android.media.MediaCodecList.REGULAR_CODECS)
                val decoders = list.codecInfos.filter { info ->
                    !info.isEncoder && info.supportedTypes.any { it.equals(MediaFormat.MIMETYPE_VIDEO_AVC, true) }
                }
                // Prefer the hardware decoder (the one the system picks first that isn't software).
                val best = decoders.firstOrNull { !it.name.startsWith("OMX.google") && !it.name.startsWith("c2.android") }
                    ?: decoders.firstOrNull() ?: return 0 to 0
                val caps = best.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC).videoCapabilities
                for ((w, h) in listOf(3840 to 2160, 2560 to 1600, 2560 to 1440, 1920 to 1200, 1920 to 1080, 1280 to 720)) {
                    if (caps.areSizeAndRateSupported(w, h, 60.0) || caps.areSizeAndRateSupported(w, h, 30.0)) return w to h
                }
            } catch (e: Exception) {
                Log.w(TAG, "couldn't read decoder limits", e)
            }
            return 0 to 0
        }
    }
}
