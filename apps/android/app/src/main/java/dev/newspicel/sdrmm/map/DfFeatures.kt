package dev.newspicel.sdrmm.map

import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.EstimateView
import dev.newspicel.sdrmm.ffi.HeatBand
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.NavPoint
import dev.newspicel.sdrmm.ffi.Ray
import dev.newspicel.sdrmm.ffi.Station
import org.maplibre.geojson.Feature
import org.maplibre.geojson.FeatureCollection
import org.maplibre.geojson.LineString
import org.maplibre.geojson.Point
import org.maplibre.geojson.Polygon
import kotlin.math.abs

object DfFeatures {
    const val WEIGHT = "weight"
    const val LEVEL = "level"
    const val CONVERGED = "converged"
    const val KIND = "kind"
    const val HEADING = "heading"
    const val ID = "id"
    const val OPACITY = "opacity"

    fun point(at: LatLon): Point = Point.fromLngLat(at.lon, at.lat)

    fun rays(rays: List<Ray>): FeatureCollection = FeatureCollection.fromFeatures(
        rays.map { ray ->
            Feature.fromGeometry(LineString.fromLngLats(listOf(point(ray.from), point(ray.to)))).apply {
                addNumberProperty(WEIGHT, ray.weight)
            }
        },
    )

    fun heat(bands: List<HeatBand>): FeatureCollection = FeatureCollection.fromFeatures(
        bands.sortedByDescending { it.level }.flatMap { band ->
            band.rings.filter { it.size >= MIN_RING }.map { ring ->
                Feature.fromGeometry(Polygon.fromLngLats(listOf(closed(ring).map(::point)))).apply {
                    addNumberProperty(LEVEL, band.level)
                    addNumberProperty(OPACITY, heatOpacity(band.level))
                }
            }
        },
    )

    fun ellipse(ring: List<LatLon>): FeatureCollection {
        if (ring.size < MIN_RING) return empty()
        return FeatureCollection.fromFeature(Feature.fromGeometry(Polygon.fromLngLats(listOf(closed(ring).map(::point)))))
    }

    fun stations(stations: List<Station>): FeatureCollection = FeatureCollection.fromFeatures(
        stations.map { station -> Feature.fromGeometry(point(station.at)).apply { addStringProperty(ID, station.id) } },
    )

    fun estimate(estimate: EstimateView?): FeatureCollection {
        estimate ?: return empty()
        return FeatureCollection.fromFeature(
            Feature.fromGeometry(point(estimate.at)).apply { addBooleanProperty(CONVERGED, estimate.converged) },
        )
    }

    fun target(target: NavPoint?): FeatureCollection {
        target ?: return empty()
        return FeatureCollection.fromFeature(
            Feature.fromGeometry(point(target.at)).apply { addStringProperty(KIND, target.kind.name.lowercase()) },
        )
    }

    fun you(
        fix: LocationSample?,
        headingDeg: Double?,
    ): FeatureCollection {
        fix ?: return empty()
        val feature = Feature.fromGeometry(Point.fromLngLat(fix.lon, fix.lat))
        headingDeg?.takeIf { it.isFinite() }?.let { feature.addNumberProperty(HEADING, it) }
        return FeatureCollection.fromFeature(feature)
    }

    fun extent(
        view: DfView?,
        fix: LocationSample?,
    ): List<LatLon> = buildList {
        view?.let {
            it.overlay.rays.forEach { ray ->
                add(ray.from)
                add(ray.to)
            }
            it.overlay.stations.forEach { station -> add(station.at) }
            it.estimate?.let { estimate -> add(estimate.at) }
            it.target?.let { target -> add(target.at) }
        }
        fix?.let { add(LatLon(it.lat, it.lon)) }
    }

    fun heatOpacity(level: Float): Float = when {
        abs(level - OUTER) < LEVEL_TOLERANCE -> 0.12f
        abs(level - MIDDLE) < LEVEL_TOLERANCE -> 0.25f
        abs(level - INNER) < LEVEL_TOLERANCE -> 0.40f
        else -> 0.5f * (1f - level) + 0.1f
    }

    fun empty(): FeatureCollection = FeatureCollection.fromFeatures(emptyList<Feature>())

    private fun closed(ring: List<LatLon>): List<LatLon> = if (ring.first() == ring.last()) ring else ring + ring.first()

    private const val MIN_RING = 3
    private const val OUTER = 0.95f
    private const val MIDDLE = 0.8f
    private const val INNER = 0.5f
    private const val LEVEL_TOLERANCE = 1e-3f
}
