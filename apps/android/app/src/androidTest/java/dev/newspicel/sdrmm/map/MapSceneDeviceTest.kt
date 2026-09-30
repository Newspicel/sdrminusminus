package dev.newspicel.sdrmm.map

import android.os.SystemClock
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.Ray
import dev.newspicel.sdrmm.settings.MapLayers
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.maplibre.android.maps.MapView
import org.maplibre.android.style.sources.GeoJsonSource
import java.util.concurrent.atomic.AtomicReference

@RunWith(AndroidJUnit4::class)
class MapSceneDeviceTest {
    @get:Rule
    val scenario = ActivityScenarioRule(MainActivity::class.java)

    @Test
    fun rays_reach_the_source() {
        val scene = AtomicReference<MapScene?>()
        val view = AtomicReference<MapView?>()
        scenario.scenario.onActivity { activity ->
            val map = MapViews.create(activity)
            view.set(map)
            activity.setContentView(map)
            MapViewSteps(map).apply {
                on(androidx.lifecycle.Lifecycle.Event.ON_CREATE)
                on(androidx.lifecycle.Lifecycle.Event.ON_START)
                on(androidx.lifecycle.Lifecycle.Event.ON_RESUME)
            }
            MapViews.load(map, dark = false, tiles = false) { scene.set(it) }
        }
        waitFor { scene.get() != null }
        val rays = (0 until 3).map { Ray(LatLon(52.5, 13.4), LatLon(52.5 + it * 0.01, 13.45), 0.3f + it * 0.2f) }
        scenario.scenario.onActivity {
            scene.get()?.showDf(Samples.df().copy(overlay = Samples.df().overlay.copy(rays = rays)), MapLayers())
            scene.get()?.fit(rays.flatMap { listOf(it.from, it.to) })
        }
        var count = 0
        waitFor {
            scenario.scenario.onActivity { activity ->
                view.get()?.getMapAsync { map ->
                    count =
                        map.style
                            ?.getSourceAs<GeoJsonSource>(MapScene.RAYS)
                            ?.querySourceFeatures(null)
                            ?.map { feature -> feature.getNumberProperty(DfFeatures.WEIGHT).toFloat() }
                            ?.distinct()
                            ?.size ?: 0
                }
            }
            count == 3
        }
        assertThat(count).isEqualTo(3)
    }

    private fun waitFor(condition: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (!condition() && SystemClock.elapsedRealtime() < deadline) Thread.sleep(STEP_MS)
    }

    private companion object {
        const val PATIENCE_MS = 10_000L
        const val STEP_MS = 200L
    }
}
