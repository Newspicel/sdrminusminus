package dev.newspicel.sdrmm.hunt

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.SweepPhase
import dev.newspicel.sdrmm.ffi.SweepView
import dev.newspicel.sdrmm.ffi.Trend
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.components.BOTTOM_ROOM
import dev.newspicel.sdrmm.ui.components.BigReadout
import dev.newspicel.sdrmm.ui.components.ChipAction
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.KeepScreenOn
import dev.newspicel.sdrmm.ui.components.StatusChips
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.components.TuneSheet
import dev.newspicel.sdrmm.ui.components.rememberMissionChipActions
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import dev.newspicel.sdrmm.ui.theme.Readout

@Composable
fun HuntScreen(
    graph: AppGraph,
    missionId: String,
) {
    val model =
        viewModel(key = missionId) {
            HuntViewModel(missionId, graph.core, graph.settings, graph.sensors, graph.runner, graph.clickTrack, graph.haptics, graph.router)
        }
    val state by model.state.collectAsStateWithLifecycle()
    val settings by graph.settings.settings.collectAsStateWithLifecycle()
    val actions = rememberMissionChipActions(graph, LocalContext.current)
    LaunchedEffect(actions) { actions.gate.request(Need.Location, fromUser = false) }
    LifecycleResumeEffect(model) {
        model.onResumed(true)
        onPauseOrDispose { model.onResumed(false) }
    }
    KeepScreenOn(settings.keepScreenOn)
    HuntContent(state, model, onBack = graph.navigator::back, onChipAction = actions::handle)
}

@Composable
fun HuntContent(
    state: HuntUiState,
    model: HuntViewModel,
    onBack: () -> Unit,
    onChipAction: (ChipAction) -> Unit,
) {
    Column(Modifier.safeDrawingPadding()) {
        TopBar(state.title, onBack = onBack)
        StatusChips(Chips.mission(state.link, state.sensors, state.pose, state.background), onChipAction, Modifier.padding(horizontal = 16.dp))
        BoxWithConstraints(Modifier.fillMaxWidth().weight(1f)) {
            if (maxWidth > maxHeight) {
                Row(Modifier.fillMaxHeight()) {
                    Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp)) { Readouts(state, model) }
                    Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp)) { Controls(state, model) }
                }
            } else {
                Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Readouts(state, model)
                    Controls(state, model)
                    Box(Modifier.height(BOTTOM_ROOM))
                }
            }
        }
    }
    if (state.tuneOpen) TuneSheet(onSet = model::tune, onDismiss = model::closeTune)
}

@Composable
private fun ColumnScope.Readouts(
    state: HuntUiState,
    model: HuntViewModel,
) {
    val view = state.view
    val tunable = MissionControl.TUNE in state.controls
    BigReadout(
        view?.let { Format.frequency(it.freqHz) } ?: "-",
        modifier = Modifier.clickable(enabled = tunable, onClick = model::openTune),
    )
    Text(Format.db(view?.smoothDb), style = Readout.copy(fontSize = MaterialTheme.typography.headlineSmall.fontSize))
    TrendRow(view?.trend ?: Trend.WAITING)
    Meter(view?.strength ?: 0f, Format.db(view?.floorDb), Format.db(view?.bestDb))
    view?.refusal?.let { Text(it, color = LocalStatusColors.current.danger) }
}

@Composable
private fun TrendRow(trend: Trend) {
    val (label, icon) =
        when (trend) {
            Trend.WAITING -> R.string.hunt_listening to R.drawable.ic_hunt
            Trend.WARMER -> R.string.hunt_warmer to R.drawable.ic_arrow_up
            Trend.COLDER -> R.string.hunt_colder to R.drawable.ic_arrow_down
            Trend.ON_TOP -> R.string.hunt_on_top to R.drawable.ic_follow
        }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Icon(painterResource(icon), contentDescription = null, tint = MaterialTheme.colorScheme.primary)
        Text(stringResource(label), style = MaterialTheme.typography.titleLarge)
    }
}

@Composable
private fun Meter(
    strength: Float,
    floor: String,
    best: String,
) {
    val fraction = if (strength.isFinite()) strength.coerceIn(0f, 1f) else 0f
    Column(Modifier.fillMaxWidth()) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(METER_HEIGHT)
                .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(8.dp))
                .semantics { stateDescription = Format.percent(fraction) },
        ) {
            Box(
                Modifier
                    .fillMaxWidth(fraction)
                    .fillMaxHeight()
                    .background(MaterialTheme.colorScheme.primary, RoundedCornerShape(8.dp)),
            )
        }
        Row(Modifier.fillMaxWidth()) {
            Text(floor, style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            Text(best, style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun ColumnScope.Controls(
    state: HuntUiState,
    model: HuntViewModel,
) {
    if (MissionControl.HUNT_RUN in state.controls) {
        Button(
            onClick = model::toggleRun,
            enabled = !state.busy,
            modifier = Modifier.fillMaxWidth().height(PRIMARY_HEIGHT),
        ) { Text(stringResource(if (state.view?.running == true) R.string.stop else R.string.start)) }
    }
    ToggleRow(stringResource(R.string.clicks), state.clicks, model::setClicks)
    ToggleRow(stringResource(R.string.haptics), state.haptics, model::setHaptics)
    if (MissionControl.TUNE in state.controls) {
        OutlinedButton(onClick = model::openTune) { Text(stringResource(R.string.tune)) }
    }
    SweepSection(state, model)
}

@Composable
private fun ToggleRow(
    label: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().clickable { onChange(!checked) }.padding(vertical = 4.dp),
    ) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

@Composable
private fun SweepSection(
    state: HuntUiState,
    model: HuntViewModel,
) {
    val canSweep = MissionControl.SWEEP in state.controls
    val sweep = state.view?.sweep
    if (!canSweep && sweep == null) return
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        FilterChip(
            selected = state.sweeping,
            enabled = canSweep && !state.busy,
            onClick = model::toggleSweep,
            label = { Text(stringResource(R.string.sweep)) },
        )
        if (MissionControl.MARK in state.controls) {
            OutlinedButton(onClick = model::mark, enabled = !state.busy) { Text(stringResource(R.string.mark)) }
        }
    }
    sweep?.takeIf { state.sweeping }?.let { SweepReadout(it, state.pose?.headingDeg) }
}

@Composable
private fun SweepReadout(
    sweep: SweepView,
    headingDeg: Double?,
) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        SweepPlot(sweep, headingDeg, stringResource(R.string.sweep), Modifier.size(SWEEP_SIZE))
        Column(Modifier.widthIn(min = 96.dp)) {
            BigReadout(Format.angle(sweep.peakDeg?.toDouble()), size = MaterialTheme.typography.headlineMedium.fontSize)
            sweep.sigmaDeg?.let { Text("±${Format.signedDegrees(it.toDouble())}", style = Readout) }
            phaseLabel(sweep.phase)?.let { Text(stringResource(it), color = LocalStatusColors.current.warn) }
        }
    }
}

fun phaseLabel(phase: SweepPhase): Int? = when (phase) {
    SweepPhase.OFF, SweepPhase.DONE -> null
    SweepPhase.IDLE -> R.string.sweep_ready
    SweepPhase.SWEEPING -> R.string.sweep_turn
    SweepPhase.NO_HEADING -> R.string.chip_no_heading
    SweepPhase.SHORT_SPAN -> R.string.sweep_short
    SweepPhase.LOW_CONTRAST -> R.string.sweep_flat
    SweepPhase.POOR_FIT -> R.string.sweep_poor_fit
    SweepPhase.HEADING_POOR -> R.string.sweep_heading_poor
    SweepPhase.TOO_FAST -> R.string.sweep_too_fast
}

private val METER_HEIGHT = 64.dp
private val PRIMARY_HEIGHT = 56.dp
private val SWEEP_SIZE = 180.dp
