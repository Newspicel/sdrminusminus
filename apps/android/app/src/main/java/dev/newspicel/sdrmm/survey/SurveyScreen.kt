package dev.newspicel.sdrmm.survey

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.map.CameraFollow
import dev.newspicel.sdrmm.map.MapHost
import dev.newspicel.sdrmm.map.MapPalette
import dev.newspicel.sdrmm.map.MapScene
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.components.BigReadout
import dev.newspicel.sdrmm.ui.components.Chip
import dev.newspicel.sdrmm.ui.components.ChipAction
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.KeepScreenOn
import dev.newspicel.sdrmm.ui.components.StatusChips
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.components.UiText
import dev.newspicel.sdrmm.ui.components.rememberMissionChipActions
import dev.newspicel.sdrmm.ui.theme.Readout
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.sample
import kotlinx.coroutines.launch

@Composable
fun SurveyScreen(
    graph: AppGraph,
    missionId: String,
) {
    val model = viewModel(key = missionId) { SurveyViewModel(missionId, graph.core, graph.sensors, graph.runner, graph.surveyTrail, graph.router) }
    val state by model.state.collectAsStateWithLifecycle()
    val settings by graph.settings.settings.collectAsStateWithLifecycle()
    val actions = rememberMissionChipActions(graph, LocalContext.current)
    LaunchedEffect(actions) { actions.gate.request(Need.Location, fromUser = false) }
    KeepScreenOn(settings.keepScreenOn)
    SurveyContent(
        state = state,
        model = model,
        onBack = graph.navigator::back,
        onChipAction = actions::handle,
        mapSlot = { modifier -> SurveyMap(model, settings.mapTiles, modifier) },
    )
}

@Composable
fun SurveyContent(
    state: SurveyUiState,
    model: SurveyViewModel,
    onBack: () -> Unit,
    onChipAction: (ChipAction) -> Unit,
    mapSlot: @Composable (Modifier) -> Unit,
) {
    Column(Modifier.safeDrawingPadding()) {
        TopBar(state.title, onBack = onBack)
        val chips =
            Chips.mission(state.link, state.sensors, state.pose, state.background) +
                listOfNotNull(
                    Chip(UiText.Res(R.string.survey_trimmed), problem = true).takeIf { state.trimmed },
                    Chips.tiles(state.tilesFailed),
                )
        StatusChips(chips, onChipAction, Modifier.padding(horizontal = 16.dp))
        Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                BigReadout(state.view?.let { Format.frequency(it.freqHz) } ?: "-", size = MaterialTheme.typography.headlineMedium.fontSize)
                BigReadout(Format.db(state.view?.levelDb), size = MaterialTheme.typography.headlineMedium.fontSize)
            }
            Legend(state.scale?.minDb, state.scale?.maxDb)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (MissionControl.SURVEY_RUN in state.controls) {
                    Button(onClick = model::toggleRecord) {
                        Text(stringResource(if (state.view?.recording == true) R.string.stop else R.string.survey_record))
                    }
                }
                OutlinedButton(onClick = model::clearTrail) { Text(stringResource(R.string.clear)) }
                OutlinedButton(onClick = model::fit) { Text(stringResource(R.string.fit)) }
            }
        }
        Box(Modifier.fillMaxWidth().weight(1f).padding(top = 8.dp)) { mapSlot(Modifier.fillMaxSize()) }
    }
}

@Composable
private fun Legend(
    minDb: Float?,
    maxDb: Float?,
) {
    val steps = MapPalette.of(isSystemInDarkTheme()).survey
    Column {
        Canvas(Modifier.fillMaxWidth().height(10.dp)) {
            val width = size.width / steps.size
            steps.forEachIndexed { index, color ->
                drawRect(Color(color), topLeft = Offset(index * width, 0f), size = Size(width, size.height))
            }
        }
        Row(Modifier.fillMaxWidth()) {
            Text(Format.db(minDb), style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            Text(Format.db(maxDb), style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@OptIn(FlowPreview::class)
@Composable
fun SurveyMap(
    model: SurveyViewModel,
    tiles: Boolean,
    modifier: Modifier,
) {
    var scene by remember { mutableStateOf<MapScene?>(null) }
    var follow by remember { mutableStateOf(CameraFollow.User) }
    MapHost(
        dark = isSystemInDarkTheme(),
        tiles = tiles,
        modifier = modifier,
        onScene = { next ->
            next?.onGesture = { follow = CameraFollow.Free }
            scene = next
        },
        onTilesFailed = model::tilesFailed,
    )
    val shown = scene ?: return
    LaunchedEffect(shown) {
        launch {
            model.state
                .map { it.runs }
                .distinctUntilChanged()
                .sample(TRAIL_MS)
                .collect(shown::showSurvey)
        }
        launch {
            model.state
                .map { it.fix to it.pose?.headingDeg }
                .distinctUntilChanged()
                .sample(YOU_MS)
                .collect { (fix, heading) ->
                    shown.showYou(fix, heading)
                    shown.follow(follow, fix, heading)
                }
        }
        model.state
            .map { it.fitRequest }
            .distinctUntilChanged()
            .drop(1)
            .collect {
                follow = if (shown.fit(model.extent())) CameraFollow.Free else CameraFollow.User
            }
    }
}

private const val TRAIL_MS = 500L
private const val YOU_MS = 100L
