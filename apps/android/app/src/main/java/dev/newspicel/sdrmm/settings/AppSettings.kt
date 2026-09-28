package dev.newspicel.sdrmm.settings

import dev.newspicel.sdrmm.ffi.HeadingMode
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.Mount
import dev.newspicel.sdrmm.ffi.PoseSettings

enum class NavChoice { GoogleMaps, Ask }

enum class Units { Auto, Metric, Imperial }

data class MapLayers(
    val rays: Boolean = true,
    val heat: Boolean = true,
    val ellipse: Boolean = true,
)

data class AppSettings(
    val activeServerId: String? = null,
    val phoneName: String,
    val headingMode: HeadingMode = HeadingMode.AUTO,
    val mount: Mount = Mount.UPRIGHT,
    val mountOffsetDeg: Double = 0.0,
    val units: Units = Units.Auto,
    val navChoice: NavChoice? = null,
    val clicks: Boolean = true,
    val haptics: Boolean = true,
    val voice: Boolean = true,
    val voiceName: String? = null,
    val keepScreenOn: Boolean = true,
    val mapTiles: Boolean = true,
    val layers: MapLayers = MapLayers(),
    val lastFix: LatLon? = null,
) {
    fun poseSettings(): PoseSettings = PoseSettings(
        headingMode = headingMode,
        mount = mount,
        mountOffsetDeg = mountOffsetDeg,
        sharePose = true,
    )
}
