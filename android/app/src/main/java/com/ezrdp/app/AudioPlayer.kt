package com.ezrdp.app

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack

/** Plays the PC's sound. Never waits: if audio falls behind, extra data is dropped to stay in sync. */
class AudioPlayer {
    private var track: AudioTrack? = null
    /** Last format the PC sent, so sound can start again after being switched off and on. */
    private var format: Pair<Int, Int>? = null
    /** Android refused to play; don't retry on every packet. Sound on/off or a reconnect tries again. */
    private var failed = false

    @Synchronized
    fun configure(sampleRate: Int, channels: Int) {
        release()
        failed = false
        format = sampleRate to channels
        val mask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
        val min = AudioTrack.getMinBufferSize(sampleRate, mask, AudioFormat.ENCODING_PCM_16BIT)
        // About 80 ms of room: enough to ride out network jitter, small enough to stay in sync.
        val size = maxOf(min, sampleRate * channels * 2 * 80 / 1000)
        // Android can refuse (e.g. during a phone call). Then just play no sound: an error here
        // would end the connection, and every reconnect would hit it again.
        track = try {
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build(),
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setSampleRate(sampleRate)
                        .setChannelMask(mask)
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .build(),
                )
                .setBufferSizeInBytes(size)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .build()
                .also { it.play() }
        } catch (e: Exception) {
            android.util.Log.w("EzRDP", "couldn't start sound", e)
            failed = true
            null
        }
    }

    @Synchronized
    fun write(data: ByteArray, offset: Int, length: Int) {
        if (track == null && !failed) format?.let { (rate, ch) -> configure(rate, ch) }
        val t = track ?: return
        t.write(data, offset, length, AudioTrack.WRITE_NON_BLOCKING)
    }

    @Synchronized
    fun release() {
        track?.let {
            try { it.stop() } catch (_: Exception) {}
            it.release()
        }
        track = null
    }
}
