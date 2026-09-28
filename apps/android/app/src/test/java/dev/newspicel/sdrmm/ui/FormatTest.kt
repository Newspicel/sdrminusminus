package dev.newspicel.sdrmm.ui

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.settings.Units
import org.junit.Test

class FormatTest {
    @Test
    fun distance() {
        assertThat(Format.distance(43.0, Units.Metric)).isEqualTo("45 m")
        assertThat(Format.distance(334.0, Units.Metric)).isEqualTo("330 m")
        assertThat(Format.distance(1234.0, Units.Metric)).isEqualTo("1.2 km")
        assertThat(Format.distance(14_200.0, Units.Metric)).isEqualTo("14 km")
        assertThat(Format.distance(Double.NaN, Units.Metric)).isEqualTo("-")
        assertThat(Format.distance(150.0, Units.Imperial)).isEqualTo("500 ft")
        assertThat(Format.distance(2000.0, Units.Imperial)).isEqualTo("1.2 mi")
        assertThat(Format.distance(20_000.0, Units.Imperial)).isEqualTo("12 mi")
        assertThat(Format.distance(Double.POSITIVE_INFINITY, Units.Imperial)).isEqualTo("-")
    }

    @Test
    fun frequency_angle_db() {
        assertThat(Format.frequency(145_500_000.0)).isEqualTo("145.500 MHz")
        assertThat(Format.frequency(433.92e6)).isEqualTo("433.920 MHz")
        assertThat(Format.frequency(2.4e9)).isEqualTo("2.4000 GHz")
        assertThat(Format.frequency(7_050.0)).isEqualTo("7.1 kHz")
        assertThat(Format.frequency(50.0)).isEqualTo("50 Hz")
        assertThat(Format.angle(7.0)).isEqualTo("007°")
        assertThat(Format.angle(360.0)).isEqualTo("000°")
        assertThat(Format.angle(-10.0)).isEqualTo("350°")
        assertThat(Format.angle(359.6)).isEqualTo("000°")
        assertThat(Format.angle(null)).isEqualTo("-")
        assertThat(Format.db(-72.44f)).isEqualTo("-72.4 dB")
        assertThat(Format.db(null)).isEqualTo("-")
        assertThat(Format.db(Float.NaN)).isEqualTo("-")
        assertThat(Format.percent(0.625f)).isEqualTo("63%")
        assertThat(Format.signedDegrees(-3.4)).isEqualTo("-3°")
        assertThat(Format.percent(Float.NaN)).isEqualTo("-")
        assertThat(Format.signedDegrees(Double.NaN)).isEqualTo("-")
    }
}
