package dev.newspicel.sdrmm.sensors

import android.hardware.GeomagneticField
import kotlin.math.asin
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.sqrt

interface DeclinationModel {
    fun declinationDeg(
        lat: Double,
        lon: Double,
        altM: Double,
        atMs: Long,
    ): Double
}

class GeomagneticDeclination : DeclinationModel {
    override fun declinationDeg(
        lat: Double,
        lon: Double,
        altM: Double,
        atMs: Long,
    ): Double = GeomagneticField(lat.toFloat(), lon.toFloat(), altM.toFloat(), atMs).declination.toDouble()
}

class Declination(
    private val model: DeclinationModel,
    private val clock: () -> Long,
) {
    @Volatile private var value: Double? = null
    private var lat = 0.0
    private var lon = 0.0
    private var atMs = 0L

    fun update(
        lat: Double,
        lon: Double,
        altM: Double,
    ) {
        val now = clock()
        val due =
            value == null ||
                now - atMs > MAX_AGE_MS ||
                distanceM(this.lat, this.lon, lat, lon) > MAX_MOVE_M
        if (!due) return
        value = model.declinationDeg(lat, lon, altM, now)
        this.lat = lat
        this.lon = lon
        atMs = now
    }

    fun current(): Double? = value

    companion object {
        const val MAX_MOVE_M = 10_000.0
        const val MAX_AGE_MS = 24L * 60 * 60 * 1000
        private const val EARTH_RADIUS_M = 6_371_000.0

        fun distanceM(
            lat1: Double,
            lon1: Double,
            lat2: Double,
            lon2: Double,
        ): Double {
            val dLat = Math.toRadians(lat2 - lat1)
            val dLon = Math.toRadians(lon2 - lon1)
            val a =
                sin(dLat / 2) * sin(dLat / 2) +
                    cos(Math.toRadians(lat1)) * cos(Math.toRadians(lat2)) * sin(dLon / 2) * sin(dLon / 2)
            return 2 * EARTH_RADIUS_M * asin(min(1.0, sqrt(a)))
        }
    }
}
