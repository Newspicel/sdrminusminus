package dev.newspicel.sdrmm.audio

import com.google.common.truth.Truth.assertThat
import org.junit.Test

class ClickShapeTest {
    @Test
    fun rate() {
        assertThat(ClickShape.rateHz(0f)).isEqualTo(1.5)
        assertThat(ClickShape.rateHz(1f)).isEqualTo(40.0)
        assertThat(ClickShape.rateHz(0.5f)).isWithin(1e-9).of(11.125)
        assertThat(ClickShape.rateHz(Float.NaN)).isEqualTo(1.5)
        assertThat(ClickShape.rateHz(7f)).isEqualTo(40.0)
    }
}
