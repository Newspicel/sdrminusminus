package dev.newspicel.sdrmm.car

import android.graphics.PixelFormat
import android.graphics.Rect
import android.media.ImageReader
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.car.app.SurfaceContainer
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.Startup
import dev.newspicel.sdrmm.settings.MapLayers
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestSdrmmApp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.atomic.AtomicInteger

@RunWith(AndroidJUnit4::class)
class CarMapRendererDeviceTest {
    private val app: TestSdrmmApp = ApplicationProvider.getApplicationContext()
    private val main = InstrumentationRegistry.getInstrumentation()

    @Test
    fun the_car_surface_gets_a_map_and_releases_it() {
        val graph = (app.startup.value as Startup.Ready).graph
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
        val frames = AtomicInteger()
        val reader = ImageReader.newInstance(WIDTH, HEIGHT, PixelFormat.RGBA_8888, 2)
        reader.setOnImageAvailableListener({ it.acquireLatestImage()?.use { frames.incrementAndGet() } }, Handler(Looper.getMainLooper()))
        lateinit var renderer: CarMapRenderer
        main.runOnMainSync {
            renderer = CarMapRenderer(app, graph, scope, dark = false)
            val owner = ProcessLifecycleOwner.get()
            renderer.onCreate(owner)
            renderer.onStart(owner)
            renderer.onResume(owner)
            renderer.onSurfaceAvailable(SurfaceContainer(reader.surface, WIDTH, HEIGHT, DPI))
            renderer.onVisibleAreaChanged(Rect(0, 80, WIDTH, HEIGHT))
        }
        waitFor { renderer.scene.value != null }
        assertThat(renderer.scene.value).isNotNull()
        main.runOnMainSync {
            renderer.scene.value?.showDf(Samples.df(), MapLayers())
            renderer.onScroll(10f, 10f)
            renderer.onScale(WIDTH / 2f, HEIGHT / 2f, 2f)
        }
        waitFor { frames.get() > 0 }
        assertThat(frames.get()).isGreaterThan(0)
        main.runOnMainSync { renderer.onSurfaceDestroyed(SurfaceContainer(reader.surface, WIDTH, HEIGHT, DPI)) }
        assertThat(renderer.scene.value).isNull()
        reader.close()
        scope.cancel()
    }

    private fun waitFor(condition: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (!condition() && SystemClock.elapsedRealtime() < deadline) Thread.sleep(STEP_MS)
    }

    private companion object {
        const val WIDTH = 800
        const val HEIGHT = 480
        const val DPI = 160
        const val PATIENCE_MS = 10_000L
        const val STEP_MS = 100L
    }
}
