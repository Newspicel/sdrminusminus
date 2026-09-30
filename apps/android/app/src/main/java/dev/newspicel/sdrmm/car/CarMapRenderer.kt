package dev.newspicel.sdrmm.car

import android.app.Presentation
import android.content.Context
import android.graphics.Rect
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import androidx.car.app.SurfaceCallback
import androidx.car.app.SurfaceContainer
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.map.CameraFollow
import dev.newspicel.sdrmm.map.MapScene
import dev.newspicel.sdrmm.map.MapViewSteps
import dev.newspicel.sdrmm.map.MapViews
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.sample
import kotlinx.coroutines.launch
import org.maplibre.android.maps.MapView
import kotlin.math.log2

class CarMapRenderer(
    private val context: Context,
    private val graph: AppGraph,
    private val scope: CoroutineScope,
    private var dark: Boolean,
) : SurfaceCallback,
    DefaultLifecycleObserver {
    private val current = MutableStateFlow<MapScene?>(null)
    private val following = MutableStateFlow(CameraFollow.UserHeading)
    private var display: VirtualDisplay? = null
    private var presentation: Presentation? = null
    private var view: MapView? = null
    private var steps: MapViewSteps? = null
    private var you: Job? = null
    private var surface: SurfaceContainer? = null
    private var visible: Rect? = null
    private var reached = Lifecycle.State.INITIALIZED
    private var tilesFailed = false

    val scene: StateFlow<MapScene?> = current.asStateFlow()
    val follow: StateFlow<CameraFollow> = following.asStateFlow()

    override fun onSurfaceAvailable(surfaceContainer: SurfaceContainer) {
        val target = surfaceContainer.surface ?: return
        release()
        surface = surfaceContainer
        val manager = context.getSystemService(DisplayManager::class.java) ?: return
        val virtual =
            manager.createVirtualDisplay(
                DISPLAY_NAME,
                surfaceContainer.width,
                surfaceContainer.height,
                surfaceContainer.dpi,
                target,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY,
            )
        display = virtual
        val shown = Presentation(context, virtual.display)
        val map = MapViews.create(shown.context)
        map.addOnDidFailLoadingMapListener {
            if (!tilesFailed) {
                tilesFailed = true
                load()
            }
        }
        shown.setContentView(map)
        shown.show()
        presentation = shown
        view = map
        steps = MapViewSteps(map).also { catchUp(it) }
        load()
    }

    private fun catchUp(steps: MapViewSteps) {
        if (reached.isAtLeast(Lifecycle.State.CREATED)) steps.on(Lifecycle.Event.ON_CREATE)
        if (reached.isAtLeast(Lifecycle.State.STARTED)) steps.on(Lifecycle.Event.ON_START)
        if (reached.isAtLeast(Lifecycle.State.RESUMED)) steps.on(Lifecycle.Event.ON_RESUME)
    }

    private fun load() {
        val map = view ?: return
        current.value?.detach()
        current.value = null
        you?.cancel()
        val tiles = graph.settings.settings.value.mapTiles && !tilesFailed && MapViews.online(context)
        MapViews.load(map, dark, tiles) { ready ->
            ready.onGesture = { following.value = CameraFollow.Free }
            visible?.let { inset(ready, it) }
            current.value = ready
            you = scope.launch { followYou(ready) }
        }
    }

    @OptIn(FlowPreview::class)
    private suspend fun followYou(ready: MapScene) {
        combine(graph.sensors.lastFix, graph.core.pose, following) { fix, pose, mode -> Triple(fix, pose?.headingDeg, mode) }
            .sample(YOU_MS)
            .collect { (fix, heading, mode) ->
                ready.showYou(fix, heading)
                ready.follow(mode, fix, heading)
            }
    }

    override fun onVisibleAreaChanged(visibleArea: Rect) {
        visible = visibleArea
        current.value?.let { inset(it, visibleArea) }
    }

    private fun inset(
        scene: MapScene,
        area: Rect,
    ) {
        val box = surface ?: return
        scene.setInsets(area.left, area.top, box.width - area.right, box.height - area.bottom)
    }

    override fun onStableAreaChanged(stableArea: Rect) {
        val box = surface ?: return
        current.value?.ornaments(stableArea.left, box.height - stableArea.bottom)
    }

    override fun onScroll(
        distanceX: Float,
        distanceY: Float,
    ) {
        following.value = CameraFollow.Free
        current.value?.scrollBy(-distanceX, -distanceY)
    }

    override fun onScale(
        focusX: Float,
        focusY: Float,
        scaleFactor: Float,
    ) {
        if (scaleFactor <= 0f || !scaleFactor.isFinite()) return
        current.value?.zoomBy(log2(scaleFactor.toDouble()), focusX, focusY)
    }

    override fun onSurfaceDestroyed(surfaceContainer: SurfaceContainer) {
        release()
    }

    fun zoom(delta: Double) {
        current.value?.zoom(delta)
    }

    fun recentre() {
        following.value = CameraFollow.UserHeading
    }

    fun free() {
        following.value = CameraFollow.Free
    }

    fun restyle(dark: Boolean) {
        if (this.dark == dark) return
        this.dark = dark
        load()
    }

    override fun onCreate(owner: LifecycleOwner) = step(Lifecycle.Event.ON_CREATE)

    override fun onStart(owner: LifecycleOwner) = step(Lifecycle.Event.ON_START)

    override fun onResume(owner: LifecycleOwner) = step(Lifecycle.Event.ON_RESUME)

    override fun onPause(owner: LifecycleOwner) = step(Lifecycle.Event.ON_PAUSE)

    override fun onStop(owner: LifecycleOwner) = step(Lifecycle.Event.ON_STOP)

    override fun onDestroy(owner: LifecycleOwner) {
        release()
        reached = Lifecycle.State.DESTROYED
    }

    private fun step(event: Lifecycle.Event) {
        reached = event.targetState
        steps?.on(event)
    }

    private fun release() {
        you?.cancel()
        you = null
        current.value?.detach()
        current.value = null
        steps?.destroy()
        steps = null
        presentation?.dismiss()
        presentation = null
        display?.release()
        display = null
        view = null
        surface = null
    }

    private companion object {
        const val DISPLAY_NAME = "sdrmm-car"
        const val YOU_MS = 100L
    }
}
