package dev.newspicel.sdrmm.sensors

import com.google.common.truth.Truth.assertThat
import org.junit.Test

class DeclinationTest {
    private class CountingModel : DeclinationModel {
        val calls = mutableListOf<Pair<Double, Double>>()

        override fun declinationDeg(
            lat: Double,
            lon: Double,
            altM: Double,
            atMs: Long,
        ): Double {
            calls += lat to lon
            return lat / 10
        }
    }

    @Test
    fun recompute_rules() {
        val model = CountingModel()
        var now = 0L
        val declination = Declination(model) { now }
        assertThat(declination.current()).isNull()
        declination.update(52.0, 13.0, 0.0)
        assertThat(model.calls).hasSize(1)
        assertThat(declination.current()).isEqualTo(5.2)
        val nineKm = 9_000.0 / 111_195.0
        declination.update(52.0 + nineKm, 13.0, 0.0)
        assertThat(model.calls).hasSize(1)
        val elevenKm = 11_000.0 / 111_195.0
        declination.update(52.0 + elevenKm, 13.0, 0.0)
        assertThat(model.calls).hasSize(2)
        now += Declination.MAX_AGE_MS + 1
        declination.update(52.0 + elevenKm, 13.0, 0.0)
        assertThat(model.calls).hasSize(3)
    }

    @Test
    fun distance_is_haversine() {
        assertThat(Declination.distanceM(0.0, 0.0, 0.0, 1.0)).isWithin(1.0).of(111_195.0)
        assertThat(Declination.distanceM(52.52, 13.405, 52.52, 13.405)).isEqualTo(0.0)
    }
}
