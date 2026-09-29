package dev.newspicel.sdrmm.ui

import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.df.DfScreen
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.hunt.HuntScreen
import dev.newspicel.sdrmm.radar.RadarScreen
import dev.newspicel.sdrmm.survey.SurveyScreen

@Composable
fun MissionScreen(
    graph: AppGraph,
    mission: Destination.Mission,
) {
    when (mission.kind) {
        MissionKind.HUNT -> HuntScreen(graph, mission.id)
        MissionKind.DF_DRIVE -> DfScreen(graph, mission.id)
        MissionKind.RADAR_WATCH -> RadarScreen(graph, mission.id)
        MissionKind.SURVEY -> SurveyScreen(graph, mission.id)
    }
}

@Composable
fun DemoStrip(graph: AppGraph) {
    if (!graph.demo.active) return
    Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = MaterialTheme.shapes.medium, modifier = Modifier.padding(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 16.dp)) {
            Text(stringResource(R.string.demo))
            TextButton(onClick = { graph.demo.demo(false) }) { Text(stringResource(R.string.demo_leave)) }
        }
    }
}
