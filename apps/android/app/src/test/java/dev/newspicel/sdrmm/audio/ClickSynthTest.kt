package dev.newspicel.sdrmm.audio

import com.google.common.truth.Truth.assertThat
import org.junit.Test
import kotlin.math.abs

class ClickSynthTest {
    private val rate = 48_000.0

    private fun render(
        strength: Float,
        seconds: Double,
    ): FloatArray {
        val synth = ClickSynth(rate)
        synth.setStrength(strength)
        val out = FloatArray((rate * seconds).toInt())
        val block = FloatArray(480)
        var at = 0
        while (at < out.size) {
            val frames = minOf(block.size, out.size - at)
            synth.render(block, frames)
            block.copyInto(out, at, 0, frames)
            at += frames
        }
        return out
    }

    private fun onsets(samples: FloatArray): List<Int> {
        val found = mutableListOf<Int>()
        var quiet = true
        var silent = 0
        samples.forEachIndexed { index, value ->
            if (value != 0f && quiet) {
                found += index - 1
                quiet = false
                silent = 0
            } else if (value == 0f) {
                silent += 1
                if (silent > GAP) quiet = true
            }
        }
        return found
    }

    @Test
    fun onsets_slowest() {
        val found = onsets(render(0f, 2.0))
        assertThat(found).hasSize(3)
        assertThat(found[0]).isAtMost(1)
        assertThat(abs(found[1] - (rate / ClickShape.SLOWEST_HZ).toInt())).isAtMost(2)
    }

    @Test
    fun onsets_40hz() {
        val found = onsets(render(1f, 2.0))
        assertThat(found.size).isIn(79..81)
    }

    @Test
    fun shape() {
        val samples = render(0f, 0.01)
        assertThat(samples[0]).isEqualTo(0f)
        val peak = samples.take(200).maxOf { abs(it) }
        assertThat(peak).isAtMost(ClickShape.PEAK)
        assertThat(peak).isGreaterThan(ClickShape.PEAK * 0.7f)
        val length = (ClickShape.CLICK_SECONDS * rate).toInt()
        assertThat(abs(samples[length - 1])).isAtMost(1.1e-4f)
        assertThat(samples.drop(length).all { it == 0f }).isTrue()
    }

    private companion object {
        const val GAP = 16
    }
}
