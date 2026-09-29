package dev.newspicel.sdrmm.radar

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ui.components.BOTTOM_ROOM
import dev.newspicel.sdrmm.ui.components.Chip
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.KeepScreenOn
import dev.newspicel.sdrmm.ui.components.SectionTitle
import dev.newspicel.sdrmm.ui.components.StatusChips
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.components.UiText
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import dev.newspicel.sdrmm.ui.theme.Readout
import java.util.Locale

@Composable
fun RadarScreen(
    graph: AppGraph,
    missionId: String,
) {
    val model = viewModel(key = missionId) { RadarViewModel(missionId, graph.core, graph.router) }
    val state by model.state.collectAsStateWithLifecycle()
    val settings by graph.settings.settings.collectAsStateWithLifecycle()
    KeepScreenOn(settings.keepScreenOn)
    RadarContent(state, onBack = graph.navigator::back)
}

@Composable
fun RadarContent(
    state: RadarUiState,
    onBack: () -> Unit,
) {
    Column(Modifier.safeDrawingPadding().verticalScroll(rememberScrollState())) {
        TopBar(state.title, onBack = onBack)
        val chips = listOfNotNull(Chips.link(state.link), Chip(UiText.Res(R.string.radar_stale), problem = true).takeIf { state.stale })
        StatusChips(chips, onAction = {}, modifier = Modifier.padding(horizontal = 16.dp))
        state.problems.forEach { Text(it, color = LocalStatusColors.current.danger, modifier = Modifier.padding(horizontal = 16.dp)) }
        Frame(state)
        SectionTitle(stringResource(R.string.radar_tracks))
        if (state.rows.isEmpty()) {
            Text(stringResource(R.string.radar_no_tracks), color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp))
        }
        state.rows.forEach { row ->
            val text = listOfNotNull(row.name, row.range, row.doppler, stringResource(row.motion), row.bearing).joinToString(SEPARATOR)
            Text(text, style = Readout, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
        }
        Text(
            pluralStringResource(R.plurals.radar_echoes, state.echoes.toInt(), state.echoes.toInt()),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(16.dp),
        )
        Box(Modifier.padding(bottom = BOTTOM_ROOM))
    }
}

@Composable
private fun Frame(state: RadarUiState) {
    Column(Modifier.padding(horizontal = 16.dp)) {
        val frame = state.frame
        if (frame == null || state.imageFailed) {
            Text(stringResource(R.string.radar_no_image), color = LocalStatusColors.current.warn, modifier = Modifier.padding(vertical = 24.dp))
            return@Column
        }
        Text(hz(state.dopplerSpanHz / 2), style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant)
        key(state.frameVersion) {
            Image(
                bitmap = frame,
                contentDescription = null,
                filterQuality = FilterQuality.None,
                contentScale = ContentScale.FillWidth,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        Text(hz(-state.dopplerSpanHz / 2), style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Row(Modifier.fillMaxWidth()) {
            Text("0 km", style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            Text(String.format(Locale.ROOT, "%.0f km", state.rangeMaxKm), style = Readout, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

private fun hz(value: Float): String = String.format(Locale.ROOT, "%+.0f Hz", value)

private const val SEPARATOR = " · "
