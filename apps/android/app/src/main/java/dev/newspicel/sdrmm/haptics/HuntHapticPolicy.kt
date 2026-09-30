package dev.newspicel.sdrmm.haptics

import dev.newspicel.sdrmm.ffi.Trend

enum class HapticCue { Increase, Decrease, Success }

class HuntHapticPolicy(
    private val minimumIntervalMs: Long = 700,
) {
    private var lastTrend: Trend? = null
    private var armed = true
    private var lastCueMs: Long? = null

    fun cue(
        trend: Trend,
        strength: Float,
        atMs: Long,
    ): HapticCue? {
        val previous = lastTrend
        lastTrend = trend
        if (strength < REARM) armed = true
        val candidate = candidate(trend, previous, strength) ?: return null
        val last = lastCueMs
        if (last != null && atMs - last < minimumIntervalMs) return null
        if (candidate == HapticCue.Success) armed = false
        lastCueMs = atMs
        return candidate
    }

    private fun candidate(
        trend: Trend,
        previous: Trend?,
        strength: Float,
    ): HapticCue? = when {
        armed && strength.isFinite() && strength >= SUCCESS -> HapticCue.Success
        trend == previous -> null
        trend == Trend.WARMER -> HapticCue.Increase
        trend == Trend.COLDER -> HapticCue.Decrease
        else -> null
    }

    private companion object {
        const val SUCCESS = 0.9f
        const val REARM = 0.8f
    }
}
