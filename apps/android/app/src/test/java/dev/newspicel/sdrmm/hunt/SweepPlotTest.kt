package dev.newspicel.sdrmm.hunt

import androidx.compose.ui.geometry.Offset
import com.google.common.truth.Truth.assertThat
import org.junit.Test

class SweepPlotTest {
    @Test
    fun bins_are_drawn_heading_up() {
        val bins = ByteArray(36) { if (it == 9) 255.toByte() else 51 }
        val bars = SweepGeometry.bars(bins, headingDeg = 90.0)
        assertThat(bars).hasSize(36)
        assertThat(bars[0].centerDeg).isWithin(1e-9).of(275.0)
        assertThat(bars[0].spanDeg).isWithin(1e-9).of(10.0)
        assertThat(bars[9].centerDeg).isWithin(1e-9).of(5.0)
        assertThat(bars[9].fraction).isEqualTo(1f)
        assertThat(bars[0].fraction).isWithin(1e-6f).of(0.2f)
    }

    @Test
    fun the_wire_uses_seventy_two_bins() {
        val bars = SweepGeometry.bars(ByteArray(72), headingDeg = null)
        assertThat(bars.first().spanDeg).isWithin(1e-9).of(5.0)
        assertThat(bars.first().centerDeg).isWithin(1e-9).of(2.5)
    }

    @Test
    fun bearing_marker_at_the_reported_angle() {
        assertThat(SweepGeometry.marker(137f, 90.0)).isWithin(1e-9).of(47.0)
        assertThat(SweepGeometry.marker(137f, null)).isWithin(1e-9).of(137.0)
        assertThat(SweepGeometry.marker(null, 90.0)).isNull()
        assertThat(SweepGeometry.marker(Float.NaN, 90.0)).isNull()
        val point = SweepGeometry.point(90.0, 10f, Offset(50f, 50f))
        assertThat(point.x).isWithin(1e-4f).of(60f)
        assertThat(point.y).isWithin(1e-4f).of(50f)
    }
}
