package dev.newspicel.sdrmm.map

import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.SurveyPoint
import kotlin.math.abs
import kotlin.math.floor

data class TrailRun(
    val id: Int,
    val bin: Int,
    val coordinates: List<LatLon>,
)

class SurveyTrail {
    private val points = ArrayDeque<SurveyPoint>()
    private val built = ArrayList<MutableRun>()
    private var nextId = 0
    private var builtMin = Float.NaN
    private var builtMax = Float.NaN
    private var rebuiltAtMs: Long? = null
    private var wasTrimmed = false

    val runs: List<TrailRun> get() = built.map { TrailRun(it.id, it.bin, it.coordinates.toList()) }
    val trimmed: Boolean get() = wasTrimmed

    fun extend(
        fresh: List<SurveyPoint>,
        minDb: Float,
        maxDb: Float,
        nowMs: Long,
    ) {
        fresh.forEach { points.addLast(it) }
        when {
            points.size > MAX_POINTS -> {
                repeat(points.size - KEPT_POINTS) { points.removeFirst() }
                wasTrimmed = true
                rebuild(minDb, maxDb, nowMs)
            }

            shouldRebuild(minDb, maxDb, nowMs) -> {
                rebuild(minDb, maxDb, nowMs)
            }

            else -> {
                fresh.forEach { append(it, builtMin, builtMax) }
            }
        }
        trimRuns()
    }

    fun clear() {
        points.clear()
        built.clear()
        builtMin = Float.NaN
        builtMax = Float.NaN
        rebuiltAtMs = null
        wasTrimmed = false
    }

    private fun shouldRebuild(
        minDb: Float,
        maxDb: Float,
        nowMs: Long,
    ): Boolean {
        val moved = changed(builtMin, minDb) || changed(builtMax, maxDb)
        val last = rebuiltAtMs
        return moved && (last == null || nowMs - last >= REBUILD_EVERY_MS)
    }

    private fun changed(
        old: Float,
        new: Float,
    ): Boolean = when {
        old.isNaN() && new.isNaN() -> false
        old.isNaN() || new.isNaN() -> true
        else -> abs(new - old) > REBUILD_DB
    }

    private fun rebuild(
        minDb: Float,
        maxDb: Float,
        nowMs: Long,
    ) {
        built.clear()
        builtMin = minDb
        builtMax = maxDb
        rebuiltAtMs = nowMs
        points.forEach { append(it, minDb, maxDb) }
    }

    private fun append(
        point: SurveyPoint,
        minDb: Float,
        maxDb: Float,
    ) {
        val bin = bin(point.levelDb, minDb, maxDb)
        val last = built.lastOrNull()
        if (last != null && last.bin == bin) {
            last.coordinates += point.at
            return
        }
        last?.coordinates?.add(point.at)
        built += MutableRun(nextId++, bin, mutableListOf(point.at))
    }

    private fun trimRuns() {
        if (built.size <= MAX_RUNS) return
        built.subList(0, built.size - MAX_RUNS).clear()
        wasTrimmed = true
    }

    private class MutableRun(
        val id: Int,
        val bin: Int,
        val coordinates: MutableList<LatLon>,
    )

    companion object {
        const val BINS = 8
        const val MAX_POINTS = 20_000
        const val KEPT_POINTS = 18_000
        const val MAX_RUNS = 1_500
        private const val REBUILD_DB = 3f
        private const val REBUILD_EVERY_MS = 2_000L
        private const val HALF_DB = 0.5f

        fun bin(
            level: Float,
            min: Float,
            max: Float,
        ): Int {
            if (!level.isFinite()) return 0
            val flat = !(max - min >= 1f)
            val low = if (flat) level - HALF_DB else min
            val high = if (flat) level + HALF_DB else max
            return floor((level - low) / (high - low) * BINS).toInt().coerceIn(0, BINS - 1)
        }
    }
}
