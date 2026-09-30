package dev.newspicel.sdrmm.df

import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.TargetMode
import dev.newspicel.sdrmm.map.CameraFollow
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.components.BigReadout
import dev.newspicel.sdrmm.ui.components.ChipAction
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.ConfirmDialog
import dev.newspicel.sdrmm.ui.components.KeepScreenOn
import dev.newspicel.sdrmm.ui.components.StatusChips
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.components.TuneSheet
import dev.newspicel.sdrmm.ui.components.rememberMissionChipActions
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import dev.newspicel.sdrmm.ui.theme.Readout

@Composable
fun DfScreen(
    graph: AppGraph,
    missionId: String,
) {
    val model =
        viewModel(key = missionId) {
            DfViewModel(missionId, graph.core, graph.settings, graph.sensors, graph.runner, graph.nav, graph.alerts, graph.router)
        }
    val state by model.state.collectAsStateWithLifecycle()
    val settings by graph.settings.settings.collectAsStateWithLifecycle()
    val activity = LocalActivity.current
    val actions = rememberMissionChipActions(graph, LocalContext.current) { _, _ -> model.permissionsChanged() }
    LaunchedEffect(actions) { actions.gate.requestInOrder(listOf(Need.Location, Need.Notifications)) }
    LifecycleResumeEffect(model) {
        model.permissionsChanged()
        onPauseOrDispose {}
    }
    KeepScreenOn(settings.keepScreenOn)
    DfContent(
        state = state,
        model = model,
        onBack = graph.navigator::back,
        onChipAction = actions::handle,
        onNavigate = { activity?.let(model::navigate) },
        mapSlot = { modifier -> DfMap(model, state.tiles, modifier) },
    )
}

@Composable
fun DfContent(
    state: DfUiState,
    model: DfViewModel,
    onBack: () -> Unit,
    onChipAction: (ChipAction) -> Unit,
    onNavigate: () -> Unit,
    mapSlot: @Composable (Modifier) -> Unit,
) {
    Column(Modifier.safeDrawingPadding()) {
        TopBar(state.title, onBack = onBack)
        val chips =
            Chips.mission(state.link, state.sensors, state.pose, state.background) +
                listOfNotNull(Chips.alerts(state.alertsOff), Chips.tiles(state.tilesFailed))
        StatusChips(chips, onChipAction, Modifier.padding(horizontal = 16.dp))
        BoxWithConstraints(Modifier.fillMaxWidth().weight(1f)) {
            val landscape = maxWidth > maxHeight
            val roseMax = if (landscape) maxHeight * ROSE_LANDSCAPE else maxHeight * ROSE_SHARE
            if (landscape) {
                Column(Modifier.fillMaxSize()) {
                    Row(Modifier.weight(1f)) {
                        Column(Modifier.weight(1f).padding(horizontal = 16.dp)) {
                            Header(state)
                            BearingRose(state.rose, Modifier.widthIn(max = roseMax).align(Alignment.CenterHorizontally))
                            GuidanceLine(state)
                        }
                        MapArea(state, model, mapSlot, Modifier.weight(1f).fillMaxHeight())
                    }
                    ActionBar(state, model, onNavigate)
                }
            } else {
                Column(Modifier.fillMaxSize()) {
                    Column(Modifier.padding(horizontal = 16.dp)) {
                        Header(state)
                        BearingRose(state.rose, Modifier.heightIn(max = roseMax).align(Alignment.CenterHorizontally))
                        GuidanceLine(state)
                    }
                    ActionBar(state, model, onNavigate)
                    MapArea(state, model, mapSlot, Modifier.weight(1f).fillMaxWidth())
                }
            }
        }
    }
    if (state.confirmClear) {
        ConfirmDialog(
            title = stringResource(R.string.clear_fusion),
            confirm = stringResource(R.string.clear),
            onConfirm = model::confirmClear,
            onDismiss = model::dismissClear,
        )
    }
    if (state.tuneOpen) TuneSheet(onSet = model::tune, onDismiss = model::closeTune)
}

@Composable
private fun Header(state: DfUiState) {
    FlowRow(verticalArrangement = Arrangement.Center, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        BigReadout(Format.angle(state.rose.bearingDeg))
        Text(
            state.view?.let { Format.percent(it.confidence) } ?: "-",
            style = Readout.copy(fontSize = MaterialTheme.typography.headlineSmall.fontSize),
            modifier = Modifier.align(Alignment.CenterVertically),
        )
        state.view?.state?.let(GuidanceText::state)?.let { label ->
            AssistChip(onClick = {}, label = { Text(stringResource(label), color = LocalStatusColors.current.warn) })
        }
        if (!state.rose.headingUp) AssistChip(onClick = {}, label = { Text(stringResource(R.string.north_up)) })
    }
}

@Composable
private fun GuidanceLine(state: DfUiState) {
    Text(
        GuidanceText.line(state.view, state.units, LocalResources.current),
        style = MaterialTheme.typography.titleMedium,
        modifier = Modifier.padding(vertical = 8.dp),
    )
}

@Composable
private fun ActionBar(
    state: DfUiState,
    model: DfViewModel,
    onNavigate: () -> Unit,
) {
    FlowRow(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        if (MissionControl.TARGET_MODE in state.controls) ModeSwitch(state, model)
        Button(onClick = onNavigate, enabled = state.view?.target != null) {
            Icon(painterResource(R.drawable.ic_navigate), contentDescription = null)
            Text(stringResource(R.string.navigate), modifier = Modifier.padding(start = 8.dp))
        }
        if (MissionControl.CALIBRATE in state.controls) OutlinedButton(onClick = model::calibrate) { Text(stringResource(R.string.calibrate)) }
        if (MissionControl.CLEAR_FUSION in state.controls) OutlinedButton(onClick = model::askClear) { Text(stringResource(R.string.clear)) }
        if (MissionControl.TUNE in state.controls) OutlinedButton(onClick = model::openTune) { Text(stringResource(R.string.tune)) }
    }
}

@Composable
private fun ModeSwitch(
    state: DfUiState,
    model: DfViewModel,
) {
    val modes = listOf(TargetMode.AUTO to R.string.mode_auto, TargetMode.DIRECT to R.string.mode_direct)
    SingleChoiceSegmentedButtonRow {
        modes.forEachIndexed { index, (mode, label) ->
            SegmentedButton(
                selected = state.view?.targetMode == mode,
                onClick = { model.setMode(mode) },
                shape = SegmentedButtonDefaults.itemShape(index, modes.size),
            ) { Text(stringResource(label)) }
        }
    }
}

@Composable
private fun MapArea(
    state: DfUiState,
    model: DfViewModel,
    mapSlot: @Composable (Modifier) -> Unit,
    modifier: Modifier,
) {
    Box(modifier) {
        mapSlot(Modifier.fillMaxSize())
        Column(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.align(Alignment.TopEnd).padding(8.dp),
        ) {
            LayersMenu(state, model)
            MapButton(onClick = model::fit) {
                Icon(painterResource(R.drawable.ic_fit), contentDescription = stringResource(R.string.fit))
            }
            MapButton(onClick = model::cycleFollow) {
                val following = state.follow != CameraFollow.Free
                Icon(
                    painterResource(R.drawable.ic_follow),
                    contentDescription = stringResource(R.string.follow),
                    tint = if (following) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun LayersMenu(
    state: DfUiState,
    model: DfViewModel,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        MapButton(onClick = { open = true }) {
            Icon(painterResource(R.drawable.ic_layers), contentDescription = stringResource(R.string.layers))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            val heat = state.view?.overlay?.heat?.isNotEmpty() == true
            LayerItem(R.string.layer_rays, state.layers.rays, enabled = true) { model.toggleLayer(MapLayer.Rays) }
            LayerItem(R.string.layer_heat, state.layers.heat, enabled = heat) { model.toggleLayer(MapLayer.Heat) }
            LayerItem(R.string.layer_ellipse, state.layers.ellipse, enabled = true) { model.toggleLayer(MapLayer.Ellipse) }
        }
    }
}

@Composable
private fun MapButton(
    onClick: () -> Unit,
    content: @Composable () -> Unit,
) {
    SmallFloatingActionButton(
        onClick = onClick,
        containerColor = MaterialTheme.colorScheme.surface,
        contentColor = MaterialTheme.colorScheme.onSurface,
        content = content,
    )
}

@Composable
private fun LayerItem(
    label: Int,
    on: Boolean,
    enabled: Boolean,
    toggle: () -> Unit,
) {
    DropdownMenuItem(
        text = { Text(stringResource(label)) },
        enabled = enabled,
        trailingIcon = { Switch(checked = on, onCheckedChange = null, enabled = enabled) },
        onClick = toggle,
    )
}

private const val ROSE_SHARE = 0.45f
private const val ROSE_LANDSCAPE = 0.7f
