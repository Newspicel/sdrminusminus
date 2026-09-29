package dev.newspicel.sdrmm.df

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.newspicel.sdrmm.map.DfFeatures
import dev.newspicel.sdrmm.map.MapHost
import dev.newspicel.sdrmm.map.MapScene
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.sample
import kotlinx.coroutines.launch

@OptIn(FlowPreview::class)
@Composable
fun DfMap(
    model: DfViewModel,
    tiles: Boolean,
    modifier: Modifier,
) {
    var scene by remember { mutableStateOf<MapScene?>(null) }
    MapHost(
        dark = isSystemInDarkTheme(),
        tiles = tiles,
        modifier = modifier,
        onScene = { next ->
            next?.onGesture = model::gesture
            scene = next
        },
        onTilesFailed = model::tilesFailed,
    )
    val shown = scene ?: return
    LaunchedEffect(shown) {
        launch {
            model.state
                .map { it.view to it.layers }
                .distinctUntilChanged()
                .sample(OVERLAY_MS)
                .collect { (view, layers) -> shown.showDf(view, layers) }
        }
        launch {
            model.state
                .map { Triple(it.fix, it.pose?.headingDeg, it.follow) }
                .distinctUntilChanged()
                .sample(YOU_MS)
                .collect { (fix, heading, follow) ->
                    shown.showYou(fix, heading)
                    shown.follow(follow, fix, heading)
                }
        }
        model.state
            .map { it.fitRequest }
            .distinctUntilChanged()
            .drop(1)
            .collect {
                val current = model.state.value
                if (!shown.fit(DfFeatures.extent(current.view, current.fix))) model.fitEmpty()
            }
    }
}

private const val OVERLAY_MS = 500L
private const val YOU_MS = 100L
