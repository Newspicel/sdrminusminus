package dev.newspicel.sdrmm.audio

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Process
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

sealed interface ClickState {
    data object Stopped : ClickState

    data object Playing : ClickState

    data class Failed(
        val code: Int,
    ) : ClickState
}

interface ClickTrack {
    val state: StateFlow<ClickState>

    fun start()

    fun stop()

    fun setStrength(strength: Float)
}

class AudioClickTrack(
    context: Context,
) : ClickTrack {
    private val sampleRate =
        context
            .getSystemService(AudioManager::class.java)
            ?.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)
            ?.toIntOrNull() ?: FALLBACK_RATE
    private val block = sampleRate / BLOCKS_PER_SECOND
    private val synth = ClickSynth(sampleRate.toDouble())
    private val current = MutableStateFlow<ClickState>(ClickState.Stopped)
    private var worker: Thread? = null

    @Volatile private var running = false

    override val state: StateFlow<ClickState> = current.asStateFlow()

    @Synchronized
    override fun start() {
        if (worker != null) return
        val track =
            try {
                build()
            } catch (_: UnsupportedOperationException) {
                current.value = ClickState.Failed(AudioTrack.ERROR)
                return
            } catch (_: IllegalArgumentException) {
                current.value = ClickState.Failed(AudioTrack.ERROR_BAD_VALUE)
                return
            }
        running = true
        worker = Thread({ loop(track) }, THREAD_NAME).also { it.start() }
        current.value = ClickState.Playing
    }

    @Synchronized
    override fun stop() {
        val thread = worker ?: return
        running = false
        worker = null
        thread.join(JOIN_MS)
        if (current.value == ClickState.Playing) current.value = ClickState.Stopped
    }

    override fun setStrength(strength: Float) {
        synth.setStrength(strength)
    }

    private fun build(): AudioTrack {
        val min = AudioTrack.getMinBufferSize(sampleRate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_FLOAT)
        return AudioTrack
            .Builder()
            .setAudioAttributes(
                AudioAttributes
                    .Builder()
                    .setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                    .build(),
            ).setAudioFormat(
                AudioFormat
                    .Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .setSampleRate(sampleRate)
                    .build(),
            ).setTransferMode(AudioTrack.MODE_STREAM)
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .setBufferSizeInBytes(maxOf(min, BUFFER_BLOCKS * block * Float.SIZE_BYTES))
            .build()
    }

    private fun loop(track: AudioTrack) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val buffer = FloatArray(block)
        track.play()
        while (running) {
            synth.render(buffer, block)
            val written = track.write(buffer, 0, block, AudioTrack.WRITE_BLOCKING)
            if (written < 0) {
                running = false
                current.value = ClickState.Failed(written)
            }
        }
        track.pause()
        track.flush()
        track.release()
    }

    private companion object {
        const val FALLBACK_RATE = 48_000
        const val BLOCKS_PER_SECOND = 100
        const val BUFFER_BLOCKS = 4
        const val JOIN_MS = 500L
        const val THREAD_NAME = "sdrmm-clicks"
    }
}
