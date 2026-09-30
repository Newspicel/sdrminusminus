package dev.newspicel.sdrmm.map

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.HeatBand
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.Ray
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Test
import org.maplibre.geojson.LineString
import org.maplibre.geojson.Point
import org.maplibre.geojson.Polygon

class DfFeaturesTest {
    private val ring = listOf(LatLon(1.0, 2.0), LatLon(1.0, 3.0), LatLon(2.0, 3.0), LatLon(1.0, 2.0))

    @Test
    fun rays_lonlat() {
        val features = DfFeatures.rays(listOf(Ray(LatLon(52.5, 13.4), LatLon(52.6, 13.5), 0.3f))).features().orEmpty()
        val line = features.single().geometry() as LineString
        assertThat(line.coordinates().map { it.longitude() to it.latitude() }).containsExactly(13.4 to 52.5, 13.5 to 52.6).inOrder()
        assertThat(features.single().getNumberProperty(DfFeatures.WEIGHT).toFloat()).isEqualTo(0.3f)
    }

    @Test
    fun heat_order() {
        val bands = listOf(HeatBand(0.5f, listOf(ring)), HeatBand(0.95f, listOf(ring)), HeatBand(0.8f, listOf(ring)))
        val features = DfFeatures.heat(bands).features().orEmpty()
        assertThat(features.map { it.getNumberProperty(DfFeatures.LEVEL).toFloat() }).containsExactly(0.95f, 0.8f, 0.5f).inOrder()
        assertThat(features.map { it.getNumberProperty(DfFeatures.OPACITY).toFloat() }).containsExactly(0.12f, 0.25f, 0.40f).inOrder()
        assertThat(DfFeatures.heatOpacity(0.6f)).isWithin(1e-6f).of(0.3f)
    }

    @Test
    fun empty_inputs() {
        assertThat(DfFeatures.estimate(null).features()).isEmpty()
        assertThat(DfFeatures.ellipse(emptyList()).features()).isEmpty()
        assertThat(DfFeatures.target(null).features()).isEmpty()
        assertThat(DfFeatures.you(null, 10.0).features()).isEmpty()
    }

    @Test
    fun open_rings_are_closed() {
        val polygon = DfFeatures.ellipse(ring.dropLast(1)).features().orEmpty().single().geometry() as Polygon
        val outer = polygon.coordinates().single()
        assertThat(outer.first()).isEqualTo(outer.last())
        assertThat(outer).hasSize(4)
    }

    @Test
    fun you_carries_heading_only_when_known() {
        val fix = Samples.fix()
        val heading = DfFeatures.you(fix, 90.0).features().orEmpty().single()
        assertThat(heading.getNumberProperty(DfFeatures.HEADING).toDouble()).isEqualTo(90.0)
        assertThat((heading.geometry() as Point).longitude()).isEqualTo(fix.lon)
        assertThat(DfFeatures.you(fix, null).features().orEmpty().single().hasProperty(DfFeatures.HEADING)).isFalse()
    }

    @Test
    fun extent_covers_every_drawn_point() {
        val view = Samples.df()
        val points = DfFeatures.extent(view, Samples.fix(1.0, 2.0))
        assertThat(points).containsAtLeast(view.overlay.rays.single().to, view.target?.at, view.estimate?.at, LatLon(1.0, 2.0))
    }
}
