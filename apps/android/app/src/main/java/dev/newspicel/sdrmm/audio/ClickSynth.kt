package dev.newspicel.sdrmm.audio

import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.PI
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.roundToInt
import kotlin.math.sin

object ClickShape {
    const val SLOWEST_HZ = 1.5
    const val FASTEST_HZ = 40.0
    const val TONE_HZ = 1800.0
    const val PEAK = 0.28f
    const val CLICK_SECONDS = 0.004
    const val FLOOR = 1e-4

    fun rateHz(strength: Float): Double {
        val c = if (strength.isFinite()) strength.coerceIn(0f, 1f).toDouble() else 0.0
        return SLOWEST_HZ + (FASTEST_HZ - SLOWEST_HZ) * c * c
    }
}

class ClickSynth(
    private val sampleRate: Double,
) {
    private val bits = AtomicInteger(0f.toRawBits())
    private val decay = exp(-1.0 / (CLICK_SECONDS_TAU * sampleRate))
    private val length = (ClickShape.CLICK_SECONDS * sampleRate).roundToInt()
    private val step = 2 * PI * ClickShape.TONE_HZ / sampleRate
    private var samplesToNext = 0.0
    private var clickPos = -1
    private var amplitude = 0f
    private var phase = 0.0

    fun setStrength(strength: Float) {
        bits.set(strength.toRawBits())
    }

    fun render(
        out: FloatArray,
        frames: Int,
    ) {
        val period = sampleRate / ClickShape.rateHz(Float.fromBits(bits.get()))
        for (i in 0 until frames) {
            if (clickPos < 0 && samplesToNext <= 0) {
                clickPos = 0
                amplitude = ClickShape.PEAK
                phase = 0.0
                samplesToNext += period
            }
            var value = 0f
            if (clickPos >= 0) {
                value = amplitude * sin(phase).toFloat()
                phase += step
                amplitude *= decay.toFloat()
                clickPos += 1
                if (clickPos >= length) clickPos = -1
            }
            samplesToNext -= 1
            out[i] = value
        }
    }

    private companion object {
        val CLICK_SECONDS_TAU = ClickShape.CLICK_SECONDS / ln(ClickShape.PEAK / ClickShape.FLOOR)
    }
}
