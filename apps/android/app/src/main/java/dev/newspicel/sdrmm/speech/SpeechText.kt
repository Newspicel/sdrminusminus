package dev.newspicel.sdrmm.speech

import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.settings.Units
import java.util.Locale
import kotlin.math.floor
import kotlin.math.roundToInt
import kotlin.math.roundToLong

object SpeechText {
    private const val FEET_PER_METER = 3.28084
    private const val METERS_PER_MILE = 1609.344
    private val COMPASS = listOf("north", "north east", "east", "south east", "south", "south west", "west", "north west")

    fun retarget(
        notice: RetargetNotice,
        distanceM: Double?,
        bearingDeg: Double?,
        units: Units,
    ): String {
        val distance = distanceM?.takeIf { it.isFinite() }?.let { spokenDistance(it, units) }
        return when {
            notice.reason == RetargetReason.KIND_CHANGED && notice.target.kind == GuidanceKind.PROBE -> {
                "Cross the bearing."
            }

            notice.reason == RetargetReason.KIND_CHANGED -> {
                sentence("Closing in.", distance)
            }

            else -> {
                val where = listOfNotNull(distance, bearingDeg?.takeIf { it.isFinite() }?.let(::compass8)).joinToString(" ")
                sentence("New target.", where.ifEmpty { null })
            }
        }
    }

    private fun sentence(
        lead: String,
        rest: String?,
    ): String = if (rest == null) lead else "$lead $rest."

    fun spokenDistance(
        m: Double,
        units: Units,
    ): String = if (units == Units.Imperial) imperial(m) else metric(m)

    private fun metric(m: Double): String = when {
        m < 100 -> named(roundTo(m, 5).toDouble(), "meter")
        m < 1000 -> named(roundTo(m, 10).toDouble(), "meter")
        m < 10_000 -> named(tenths(m / 1000), "kilometer")
        else -> named((m / 1000).roundToLong().toDouble(), "kilometer")
    }

    private fun imperial(m: Double): String {
        val ft = m * FEET_PER_METER
        if (ft < 1000) return named(roundTo(ft, 50).toDouble(), "foot", "feet")
        val mi = m / METERS_PER_MILE
        return if (mi < 10) named(tenths(mi), "mile") else named(mi.roundToLong().toDouble(), "mile")
    }

    private fun named(
        value: Double,
        unit: String,
        plural: String = "${unit}s",
    ): String {
        val whole = value == floor(value)
        val number = if (whole) value.toLong().toString() else String.format(Locale.ROOT, "%.1f", value)
        return "$number ${if (whole && value.toLong() == 1L) unit else plural}"
    }

    private fun tenths(value: Double): Double = (value * 10).roundToLong() / 10.0

    private fun roundTo(
        value: Double,
        step: Int,
    ): Long = floor(value / step + 0.5).toLong() * step

    fun compass8(bearingDeg: Double): String {
        val wrapped = (bearingDeg % 360 + 360) % 360
        return COMPASS[(wrapped / 45).roundToInt() % COMPASS.size]
    }
}
