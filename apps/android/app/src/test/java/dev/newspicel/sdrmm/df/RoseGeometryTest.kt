package dev.newspicel.sdrmm.df

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.GuidanceView
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Test

class RoseGeometryTest {
    @Test
    fun heading_up() {
        val view = Samples.df(bearingRel = 30f, guidance = GuidanceView(GuidanceKind.PROBE, 50.0, -40.0, 900.0))
        val model = RoseGeometry.model(view, Samples.pose(heading = 90.0))
        assertThat(model.headingUp).isTrue()
        assertThat(model.bearingDeg).isWithin(1e-9).of(30.0)
        assertThat(model.guidanceDeg).isWithin(1e-9).of(320.0)
        assertThat(model.northDeg).isWithin(1e-9).of(270.0)
    }

    @Test
    fun north_up() {
        val view = Samples.df(bearingTrue = 137f, guidance = GuidanceView(GuidanceKind.PROBE, 215.0, null, 900.0))
        val model = RoseGeometry.model(view, Samples.pose(heading = null))
        assertThat(model.headingUp).isFalse()
        assertThat(model.northDeg).isEqualTo(0.0)
        assertThat(model.bearingDeg).isWithin(1e-9).of(137.0)
        assertThat(model.guidanceDeg).isWithin(1e-9).of(215.0)
        assertThat(RoseGeometry.model(view.copy(bearingRelDeg = null), Samples.pose(heading = 90.0)).headingUp).isFalse()
    }

    @Test
    fun not_live() {
        val model = RoseGeometry.model(Samples.df(state = DfState.CALIBRATING), Samples.pose())
        assertThat(model.bearingDeg).isNull()
        assertThat(RoseGeometry.model(null, null).bearingDeg).isNull()
    }

    @Test
    fun point() {
        val right = RoseGeometry.point(90.0, 10f, 50f, 50f)
        assertThat(right.x).isWithin(1e-4f).of(60f)
        assertThat(right.y).isWithin(1e-4f).of(50f)
        val up = RoseGeometry.point(0.0, 10f, 50f, 50f)
        assertThat(up.y).isWithin(1e-4f).of(40f)
    }

    @Test
    fun side_text() {
        assertThat(sideText(40.0)).isEqualTo("40° right")
        assertThat(sideText(320.0)).isEqualTo("40° left")
        assertThat(sideText(0.2)).isEqualTo("ahead")
    }
}
