package dev.newspicel.sdrmm.map

import org.maplibre.geojson.Feature
import org.maplibre.geojson.FeatureCollection
import org.maplibre.geojson.LineString

object SurveyFeatures {
    const val BIN = "bin"

    fun lines(runs: List<TrailRun>): FeatureCollection = FeatureCollection.fromFeatures(
        runs.filter { it.coordinates.size >= 2 }.map { run ->
            Feature.fromGeometry(LineString.fromLngLats(run.coordinates.map(DfFeatures::point))).apply {
                addNumberProperty(BIN, run.bin)
            }
        },
    )
}
