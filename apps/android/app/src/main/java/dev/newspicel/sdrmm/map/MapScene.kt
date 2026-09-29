package dev.newspicel.sdrmm.map

import android.graphics.Bitmap
import android.graphics.Point
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.settings.MapLayers
import org.maplibre.android.camera.CameraPosition
import org.maplibre.android.camera.CameraUpdateFactory
import org.maplibre.android.geometry.LatLng
import org.maplibre.android.geometry.LatLngBounds
import org.maplibre.android.maps.MapLibreMap
import org.maplibre.android.maps.Style
import org.maplibre.android.style.expressions.Expression
import org.maplibre.android.style.layers.CircleLayer
import org.maplibre.android.style.layers.FillLayer
import org.maplibre.android.style.layers.Layer
import org.maplibre.android.style.layers.LineLayer
import org.maplibre.android.style.layers.Property
import org.maplibre.android.style.layers.PropertyFactory
import org.maplibre.android.style.layers.SymbolLayer
import org.maplibre.android.style.sources.GeoJsonSource
import org.maplibre.geojson.FeatureCollection
import kotlin.math.min

enum class CameraFollow { Free, User, UserHeading }

class MapScene(
    private val map: MapLibreMap,
    private val style: Style,
    private val palette: MapPalette,
    youIcon: Bitmap?,
) {
    var onGesture: () -> Unit = {}
    private var placed = false
    private val gestures =
        MapLibreMap.OnCameraMoveStartedListener { reason ->
            if (reason == MapLibreMap.OnCameraMoveStartedListener.REASON_API_GESTURE) onGesture()
        }

    init {
        SOURCES.forEach { style.addSource(GeoJsonSource(it, DfFeatures.empty())) }
        youIcon?.let { style.addImage(MapIcons.YOU_ARROW, it) }
        layers().forEach(style::addLayer)
        map.setMinZoomPreference(MIN_ZOOM)
        map.setMaxZoomPreference(MAX_ZOOM)
        map.addOnCameraMoveStartedListener(gestures)
    }

    fun detach() {
        map.removeOnCameraMoveStartedListener(gestures)
    }

    fun showDf(
        view: DfView?,
        layers: MapLayers,
    ) {
        val overlay = view?.overlay
        set(RAYS, DfFeatures.rays(overlay?.rays.orEmpty()))
        set(HEAT, DfFeatures.heat(overlay?.heat.orEmpty()))
        set(ELLIPSE, DfFeatures.ellipse(overlay?.ellipse.orEmpty()))
        set(STATIONS, DfFeatures.stations(overlay?.stations.orEmpty()))
        set(ESTIMATE, DfFeatures.estimate(view?.estimate))
        set(TARGET, DfFeatures.target(view?.target))
        visible(RAYS, layers.rays)
        visible(HEAT, layers.heat)
        visible(ELLIPSE_FILL, layers.ellipse)
        visible(ELLIPSE_LINE, layers.ellipse)
    }

    fun showSurvey(runs: List<TrailRun>) {
        set(SURVEY, SurveyFeatures.lines(runs))
    }

    fun showYou(
        fix: LocationSample?,
        headingDeg: Double?,
    ) {
        set(YOU, DfFeatures.you(fix, headingDeg))
    }

    fun follow(
        mode: CameraFollow,
        fix: LocationSample?,
        headingDeg: Double?,
    ) {
        fix ?: return
        if (mode == CameraFollow.Free) return
        val current = map.cameraPosition
        val builder = CameraPosition.Builder(current).target(LatLng(fix.lat, fix.lon))
        if (!placed) builder.zoom(FOLLOW_ZOOM)
        placed = true
        builder.bearing(if (mode == CameraFollow.User) 0.0 else headingDeg ?: current.bearing)
        map.easeCamera(CameraUpdateFactory.newCameraPosition(builder.build()), EASE_MS)
    }

    fun fit(points: List<LatLon>): Boolean {
        if (points.isEmpty()) return false
        placed = true
        if (points.size == 1 || points.distinct().size == 1) {
            map.easeCamera(CameraUpdateFactory.newLatLngZoom(LatLng(points[0].lat, points[0].lon), SINGLE_ZOOM), EASE_MS)
            return true
        }
        val bounds = LatLngBounds.Builder().includes(points.map { LatLng(it.lat, it.lon) }).build()
        val padding = (min(map.width, map.height) * FIT_PADDING).toInt()
        map.easeCamera(CameraUpdateFactory.newLatLngBounds(bounds, padding), EASE_MS)
        return true
    }

    fun zoomBy(
        delta: Double,
        focusX: Float,
        focusY: Float,
    ) {
        map.moveCamera(CameraUpdateFactory.zoomBy(delta, Point(focusX.toInt(), focusY.toInt())))
    }

    fun zoom(delta: Double) {
        map.easeCamera(CameraUpdateFactory.zoomBy(delta), EASE_MS)
    }

    fun scrollBy(
        dx: Float,
        dy: Float,
    ) {
        map.scrollBy(dx, dy)
    }

    fun setInsets(
        left: Int,
        top: Int,
        right: Int,
        bottom: Int,
    ) {
        map.moveCamera(CameraUpdateFactory.paddingTo(left.toDouble(), top.toDouble(), right.toDouble(), bottom.toDouble()))
    }

    fun ornaments(
        left: Int,
        bottom: Int,
    ) {
        map.uiSettings.setAttributionMargins(left + ORNAMENT_GAP + LOGO_ROOM, 0, 0, bottom + ORNAMENT_GAP)
        map.uiSettings.setLogoMargins(left + ORNAMENT_GAP, 0, 0, bottom + ORNAMENT_GAP)
    }

    fun clear() {
        SOURCES.forEach { set(it, DfFeatures.empty()) }
    }

    private fun set(
        id: String,
        features: FeatureCollection,
    ) {
        if (style.isFullyLoaded) style.getSourceAs<GeoJsonSource>(id)?.setGeoJson(features)
    }

    private fun visible(
        id: String,
        on: Boolean,
    ) {
        if (style.isFullyLoaded) style.getLayer(id)?.setProperties(PropertyFactory.visibility(if (on) Property.VISIBLE else Property.NONE))
    }

    private fun layers(): List<Layer> = listOf(
        FillLayer(HEAT, HEAT).withProperties(
            PropertyFactory.fillColor(palette.heat),
            PropertyFactory.fillOpacity(Expression.get(DfFeatures.OPACITY)),
        ),
        LineLayer(SURVEY, SURVEY).withProperties(
            PropertyFactory.lineWidth(SURVEY_WIDTH),
            PropertyFactory.lineCap(Property.LINE_CAP_ROUND),
            PropertyFactory.lineJoin(Property.LINE_JOIN_ROUND),
            PropertyFactory.lineColor(surveyColor()),
        ),
        LineLayer(RAYS, RAYS).withProperties(
            PropertyFactory.lineColor(palette.accent),
            PropertyFactory.lineWidth(LINE_WIDTH),
            PropertyFactory.lineOpacity(Expression.max(Expression.literal(MIN_RAY_OPACITY), Expression.get(DfFeatures.WEIGHT))),
        ),
        FillLayer(ELLIPSE_FILL, ELLIPSE).withProperties(
            PropertyFactory.fillColor(palette.accent),
            PropertyFactory.fillOpacity(ELLIPSE_OPACITY),
        ),
        LineLayer(ELLIPSE_LINE, ELLIPSE).withProperties(
            PropertyFactory.lineColor(palette.accent),
            PropertyFactory.lineWidth(LINE_WIDTH),
        ),
        CircleLayer(STATIONS, STATIONS).withProperties(
            PropertyFactory.circleColor(palette.station),
            PropertyFactory.circleRadius(STATION_RADIUS),
        ),
        CircleLayer(ESTIMATE, ESTIMATE).withProperties(
            PropertyFactory.circleColor(palette.accent),
            PropertyFactory.circleRadius(ESTIMATE_RADIUS),
            PropertyFactory.circleOpacity(
                Expression.switchCase(Expression.get(DfFeatures.CONVERGED), Expression.literal(1f), Expression.literal(0f)),
            ),
            PropertyFactory.circleStrokeColor(palette.accent),
            PropertyFactory.circleStrokeWidth(LINE_WIDTH),
        ),
        CircleLayer(TARGET, TARGET).withProperties(
            PropertyFactory.circleColor(palette.warn),
            PropertyFactory.circleRadius(TARGET_RADIUS),
            PropertyFactory.circleStrokeColor(palette.ink),
            PropertyFactory.circleStrokeWidth(TARGET_STROKE),
        ),
        SymbolLayer(YOU, YOU)
            .withProperties(
                PropertyFactory.iconImage(MapIcons.YOU_ARROW),
                PropertyFactory.iconRotate(Expression.get(DfFeatures.HEADING)),
                PropertyFactory.iconRotationAlignment(Property.ICON_ROTATION_ALIGNMENT_MAP),
                PropertyFactory.iconAllowOverlap(true),
                PropertyFactory.iconIgnorePlacement(true),
            ).withFilter(Expression.has(DfFeatures.HEADING)),
        CircleLayer(YOU_DOT, YOU)
            .withProperties(
                PropertyFactory.circleColor(palette.accent),
                PropertyFactory.circleRadius(YOU_RADIUS),
                PropertyFactory.circleStrokeColor(palette.background),
                PropertyFactory.circleStrokeWidth(TARGET_STROKE),
            ).withFilter(Expression.not(Expression.has(DfFeatures.HEADING))),
    )

    private fun surveyColor(): Expression = Expression.match(
        Expression.toNumber(Expression.get(SurveyFeatures.BIN)),
        Expression.color(palette.survey.first()),
        *palette.survey.mapIndexed { bin, color -> Expression.stop(bin, Expression.color(color)) }.toTypedArray(),
    )

    companion object {
        const val HEAT = "heat"
        const val SURVEY = "survey"
        const val RAYS = "rays"
        const val ELLIPSE = "ellipse"
        const val ELLIPSE_FILL = "ellipse-fill"
        const val ELLIPSE_LINE = "ellipse-line"
        const val STATIONS = "stations"
        const val ESTIMATE = "estimate"
        const val TARGET = "target"
        const val YOU = "you"
        const val YOU_DOT = "you-dot"
        val SOURCES = listOf(HEAT, SURVEY, RAYS, ELLIPSE, STATIONS, ESTIMATE, TARGET, YOU)
        const val MIN_ZOOM = 4.0
        const val MAX_ZOOM = 19.0
        private const val SINGLE_ZOOM = 15.0
        private const val FOLLOW_ZOOM = 14.0
        private const val FIT_PADDING = 0.15f
        private const val EASE_MS = 250
        private const val LINE_WIDTH = 2f
        private const val SURVEY_WIDTH = 4f
        private const val MIN_RAY_OPACITY = 0.1f
        private const val ELLIPSE_OPACITY = 0.15f
        private const val STATION_RADIUS = 4f
        private const val ESTIMATE_RADIUS = 7f
        private const val TARGET_RADIUS = 6f
        private const val TARGET_STROKE = 1.5f
        private const val YOU_RADIUS = 6f
        private const val ORNAMENT_GAP = 8
        private const val LOGO_ROOM = 96
    }
}
