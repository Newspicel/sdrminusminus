package dev.newspicel.sdrmm.ui

import android.icu.util.LocaleData
import android.icu.util.ULocale
import dev.newspicel.sdrmm.settings.Units
import java.util.Locale
import kotlin.math.floor
import kotlin.math.roundToInt
import kotlin.math.roundToLong

object Format {
    private const val FEET_PER_METER = 3.28084
    private const val METERS_PER_MILE = 1609.344

    fun distance(
        m: Double,
        units: Units,
    ): String {
        if (!m.isFinite()) return "-"
        return if (units == Units.Imperial) imperial(m) else metric(m)
    }

    private fun metric(m: Double): String = when {
        m < 100 -> "${roundTo(m, 5)} m"
        m < 1000 -> "${roundTo(m, 10)} m"
        m < 10_000 -> String.format(Locale.ROOT, "%.1f km", m / 1000)
        else -> "${(m / 1000).roundToLong()} km"
    }

    private fun imperial(m: Double): String {
        val ft = m * FEET_PER_METER
        if (ft < 1000) return "${roundTo(ft, 50)} ft"
        val mi = m / METERS_PER_MILE
        return if (mi < 10) String.format(Locale.ROOT, "%.1f mi", mi) else "${mi.roundToLong()} mi"
    }

    private fun roundTo(
        value: Double,
        step: Int,
    ): Long = floor(value / step + 0.5).toLong() * step

    fun frequency(hz: Double): String = when {
        !hz.isFinite() -> "-"
        hz >= 1e9 -> String.format(Locale.ROOT, "%.4f GHz", hz / 1e9)
        hz >= 1e6 -> String.format(Locale.ROOT, "%.3f MHz", hz / 1e6)
        hz >= 1e3 -> String.format(Locale.ROOT, "%.1f kHz", hz / 1e3)
        else -> String.format(Locale.ROOT, "%.0f Hz", hz)
    }

    fun angle(deg: Double?): String {
        if (deg == null || !deg.isFinite()) return "-"
        val whole = ((deg % 360 + 360) % 360).roundToInt() % 360
        return String.format(Locale.ROOT, "%03d°", whole)
    }

    fun db(db: Float?): String {
        if (db == null || !db.isFinite()) return "-"
        return String.format(Locale.ROOT, "%.1f dB", db)
    }

    fun signedDegrees(deg: Double): String = if (deg.isFinite()) "${deg.roundToInt()}°" else "-"

    fun percent(fraction: Float): String = if (fraction.isFinite()) "${(fraction * 100).roundToInt()}%" else "-"

    fun resolve(units: Units): Units = if (units != Units.Auto) {
        units
    } else {
        when (LocaleData.getMeasurementSystem(ULocale.getDefault())) {
            LocaleData.MeasurementSystem.US, LocaleData.MeasurementSystem.UK -> Units.Imperial
            else -> Units.Metric
        }
    }
}
