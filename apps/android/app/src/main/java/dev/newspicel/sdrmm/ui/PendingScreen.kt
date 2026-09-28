package dev.newspicel.sdrmm.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionGate
import dev.newspicel.sdrmm.permissions.rememberPermissionGate
import dev.newspicel.sdrmm.ui.components.ChipAction
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.StatusChips
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors

@Composable
fun PendingScreen(
    graph: AppGraph,
    title: String,
) {
    val status by graph.sensors.status.collectAsStateWithLifecycle()
    val pose by graph.core.pose.collectAsStateWithLifecycle()
    val link by graph.core.link.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val gate = rememberPermissionGate { _, _ -> graph.permissionsChanged() }
    LaunchedEffect(gate) { gate.requestInOrder(listOf(Need.LocalNetwork, Need.Location)) }
    val root = graph.navigator.backStack.size <= 1
    Column(Modifier.safeDrawingPadding()) {
        TopBar(title, onBack = graph.navigator::back.takeIf { !root }) {
            IconButton(onClick = { graph.navigator.push(Destination.Settings) }) {
                Icon(painterResource(R.drawable.ic_settings), contentDescription = stringResource(R.string.settings_title))
            }
        }
        StatusChips(
            chips = Chips.of(link, status, pose),
            onAction = { action ->
                when (action) {
                    ChipAction.AllowLocation -> gate.request(Need.Location)
                    ChipAction.LocationSettings -> PermissionGate.openLocationSettings(context)
                }
            },
            modifier = Modifier.padding(horizontal = 16.dp),
        )
        Text(
            stringResource(R.string.not_built),
            color = LocalStatusColors.current.warn,
            modifier = Modifier.padding(16.dp),
        )
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

fun missionTitle(
    graph: AppGraph,
    id: String,
): String = graph.core.missions.value?.missions?.firstOrNull { it.id == id }?.title ?: id
