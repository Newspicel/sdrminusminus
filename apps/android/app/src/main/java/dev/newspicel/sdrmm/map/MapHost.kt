package dev.newspicel.sdrmm.map

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.view.Gravity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import org.maplibre.android.MapLibre
import org.maplibre.android.maps.MapLibreMapOptions
import org.maplibre.android.maps.MapView
import org.maplibre.android.maps.Style

@Composable
fun MapHost(
    dark: Boolean,
    tiles: Boolean,
    modifier: Modifier,
    onScene: (MapScene?) -> Unit,
    onTilesFailed: () -> Unit,
) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val sceneChanged by rememberUpdatedState(onScene)
    val slot = remember { SceneSlot() }
    val tilesFailed by rememberUpdatedState(onTilesFailed)
    var failed by remember { mutableStateOf(false) }
    val view = remember { MapViews.create(context) }
    val steps = remember(view) { MapViewSteps(view) }
    DisposableEffect(lifecycle, view) {
        val observer = LifecycleEventObserver { _, event -> steps.on(event) }
        view.addOnDidFailLoadingMapListener {
            failed = true
            tilesFailed()
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            slot.replace(null)
            sceneChanged(null)
            steps.destroy()
        }
    }
    LaunchedEffect(view, dark, tiles, failed) {
        slot.replace(null)
        sceneChanged(null)
        if (tiles && !failed && !MapViews.online(view.context)) {
            failed = true
            tilesFailed()
            return@LaunchedEffect
        }
        MapViews.load(view, dark, tiles && !failed) { scene ->
            slot.replace(scene)
            sceneChanged(scene)
        }
    }
    AndroidView(factory = { view }, modifier = modifier)
}

object MapViews {
    fun create(context: Context): MapView {
        MapLibre.getInstance(context)
        val options =
            MapLibreMapOptions
                .createFromAttributes(context)
                .textureMode(false)
                .compassGravity(Gravity.TOP or Gravity.START)
        return MapView(context, options)
    }

    fun online(context: Context): Boolean {
        val manager = context.getSystemService(ConnectivityManager::class.java) ?: return false
        val capabilities = manager.getNetworkCapabilities(manager.activeNetwork) ?: return false
        return capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    fun load(
        view: MapView,
        dark: Boolean,
        tiles: Boolean,
        ready: (MapScene) -> Unit,
    ) {
        val palette = MapPalette.of(dark)
        val builder = if (tiles) Style.Builder().fromUri(MapStyles.url(dark)) else Style.Builder().fromJson(MapStyles.blank(palette.background))
        view.getMapAsync { map ->
            map.setStyle(builder) { style -> ready(MapScene(map, style, palette, MapIcons.you(view.context, palette.accent))) }
        }
    }
}

class SceneSlot {
    private var current: MapScene? = null

    fun replace(scene: MapScene?) {
        current?.detach()
        current = scene
    }
}

class MapViewSteps(
    private val view: MapView,
) {
    private var reached = Lifecycle.State.INITIALIZED

    fun on(event: Lifecycle.Event) {
        when (event) {
            Lifecycle.Event.ON_CREATE -> view.onCreate(null)
            Lifecycle.Event.ON_START -> view.onStart()
            Lifecycle.Event.ON_RESUME -> view.onResume()
            Lifecycle.Event.ON_PAUSE -> view.onPause()
            Lifecycle.Event.ON_STOP -> view.onStop()
            Lifecycle.Event.ON_DESTROY -> view.onDestroy()
            Lifecycle.Event.ON_ANY -> return
        }
        reached = event.targetState
    }

    fun destroy() {
        if (reached.isAtLeast(Lifecycle.State.RESUMED)) on(Lifecycle.Event.ON_PAUSE)
        if (reached.isAtLeast(Lifecycle.State.STARTED)) on(Lifecycle.Event.ON_STOP)
        if (reached.isAtLeast(Lifecycle.State.CREATED)) on(Lifecycle.Event.ON_DESTROY)
    }
}
