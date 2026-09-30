package dev.newspicel.sdrmm.map

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.SurveyPoint
import org.junit.Test

class SurveyTrailTest {
    private fun point(
        index: Int,
        level: Float,
    ) = SurveyPoint(LatLon(52.0 + index * 1e-4, 13.0), level)

    @Test
    fun bins() {
        assertThat(SurveyTrail.bin(-90f, -90f, -50f)).isEqualTo(0)
        assertThat(SurveyTrail.bin(-50f, -90f, -50f)).isEqualTo(7)
        assertThat(SurveyTrail.bin(-70f, -90f, -50f)).isEqualTo(4)
        assertThat(SurveyTrail.bin(-70f, -70f, -69.5f)).isEqualTo(4)
        assertThat(SurveyTrail.bin(-70f, Float.NaN, Float.NaN)).isEqualTo(4)
    }

    @Test
    fun runs_share_boundary() {
        val trail = SurveyTrail()
        trail.extend(listOf(point(0, -90f), point(1, -90f), point(2, -50f), point(3, -50f)), -90f, -50f, 0)
        val runs = trail.runs
        assertThat(runs).hasSize(2)
        assertThat(runs[0].bin).isEqualTo(0)
        assertThat(runs[1].bin).isEqualTo(7)
        assertThat(runs[0].coordinates.last()).isEqualTo(runs[1].coordinates.first())
        assertThat(runs[0].coordinates).hasSize(3)
    }

    @Test
    fun trim() {
        val trail = SurveyTrail()
        val points = (0..SurveyTrail.MAX_RUNS + 10).map { point(it, if (it % 2 == 0) -90f else -50f) }
        trail.extend(points, -90f, -50f, 0)
        assertThat(trail.runs).hasSize(SurveyTrail.MAX_RUNS)
        assertThat(trail.trimmed).isTrue()
        assertThat(trail.runs.first().id).isGreaterThan(0)
        trail.clear()
        assertThat(trail.runs).isEmpty()
        assertThat(trail.trimmed).isFalse()
    }

    @Test
    fun a_moved_range_rebuilds_at_most_every_two_seconds() {
        val trail = SurveyTrail()
        trail.extend(listOf(point(0, -70f)), -90f, -50f, 0)
        assertThat(trail.runs.single().bin).isEqualTo(4)
        trail.extend(listOf(point(1, -70f)), -70f, -60f, 1_000)
        assertThat(trail.runs.map { it.bin }).containsExactly(4)
        assertThat(trail.runs.single().coordinates).hasSize(2)
        trail.extend(listOf(point(2, -70f)), -70f, -60f, 2_500)
        assertThat(trail.runs.map { it.bin }).containsExactly(0)
        assertThat(trail.runs.single().coordinates).hasSize(3)
    }

    @Test
    fun a_long_walk_keeps_the_newest_points() {
        val trail = SurveyTrail()
        val walk = (0 until SurveyTrail.MAX_POINTS + 1).map { point(it, -70f) }
        trail.extend(walk.take(SurveyTrail.MAX_POINTS), -90f, -50f, 0)
        assertThat(trail.trimmed).isFalse()
        assertThat(trail.runs.single().coordinates).hasSize(SurveyTrail.MAX_POINTS)
        trail.extend(walk.takeLast(1), -90f, -50f, 100)
        assertThat(trail.trimmed).isTrue()
        val kept = trail.runs.single().coordinates
        assertThat(kept).hasSize(SurveyTrail.KEPT_POINTS)
        assertThat(kept.last()).isEqualTo(walk.last().at)
        assertThat(kept.first()).isEqualTo(walk[walk.size - SurveyTrail.KEPT_POINTS].at)
    }
}
