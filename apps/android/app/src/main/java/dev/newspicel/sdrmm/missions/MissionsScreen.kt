package dev.newspicel.sdrmm.missions

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionGate
import dev.newspicel.sdrmm.permissions.rememberPermissionGate
import dev.newspicel.sdrmm.ui.Destination
import dev.newspicel.sdrmm.ui.components.BOTTOM_ROOM
import dev.newspicel.sdrmm.ui.components.ChipAction
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.ConfirmDialog
import dev.newspicel.sdrmm.ui.components.SectionTitle
import dev.newspicel.sdrmm.ui.components.StatusChips
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors

@Composable
fun MissionsScreen(graph: AppGraph) {
    val model = viewModel { MissionsViewModel(graph.core, graph.settings, graph.sensors, graph.navigator, graph.router) }
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val gate = rememberPermissionGate { _, _ -> graph.permissionsChanged() }
    LaunchedEffect(gate) { gate.requestInOrder(listOf(Need.LocalNetwork, Need.Location)) }
    MissionsContent(
        state = state,
        model = model,
        onSettings = { graph.navigator.push(Destination.Settings) },
        onChipAction = { action ->
            when (action) {
                ChipAction.AllowLocation -> gate.request(Need.Location)
                ChipAction.LocationSettings -> PermissionGate.openLocationSettings(context)
                ChipAction.AllowNotifications -> gate.request(Need.Notifications)
            }
        },
    )
}

@Composable
fun MissionsContent(
    state: MissionsUiState,
    model: MissionsViewModel,
    onSettings: () -> Unit,
    onChipAction: (ChipAction) -> Unit,
) {
    Column(Modifier.safeDrawingPadding()) {
        TopBar(state.serverName.ifEmpty { stringResource(R.string.missions_title) }) {
            WorkspaceMenu(state, model)
            IconButton(onClick = onSettings) {
                Icon(painterResource(R.drawable.ic_settings), contentDescription = stringResource(R.string.settings_title))
            }
        }
        LinkRow(state.link, onReconnect = model::reconnect)
        StatusChips(
            chips = listOfNotNull(Chips.sharing(state.pose), Chips.location(state.sensors), Chips.heading(state.pose)),
            onAction = onChipAction,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
        PullToRefreshBox(isRefreshing = state.refreshing, onRefresh = model::refresh, modifier = Modifier.fillMaxSize()) {
            MissionList(state, model::open)
        }
    }
    state.confirmWorkspace?.let {
        ConfirmDialog(
            title = stringResource(R.string.switch_workspace),
            confirm = stringResource(R.string.switch_button),
            onConfirm = model::confirmWorkspace,
            onDismiss = model::dismissWorkspace,
        )
    }
}

@Composable
private fun MissionList(
    state: MissionsUiState,
    open: (Mission) -> Unit,
) {
    LazyColumn(Modifier.fillMaxSize()) {
        if (state.sections.isEmpty()) {
            item {
                Text(
                    stringResource(R.string.missions_empty),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(16.dp),
                )
            }
        }
        state.sections.forEach { section ->
            item(key = "section-${section.kind}") { SectionTitle(stringResource(kindLabel(section.kind))) }
            items(section.missions, key = { it.id }) { mission -> MissionRow(mission, open) }
        }
        item { Spacer(Modifier.padding(bottom = BOTTOM_ROOM)) }
    }
}

@Composable
private fun MissionRow(
    mission: Mission,
    open: (Mission) -> Unit,
) {
    ListItem(
        headlineContent = { Text(mission.title) },
        supportingContent = { Text(mission.detail) },
        trailingContent = mission.blocker?.takeIf { !mission.ready }?.let { { Text(it, color = LocalStatusColors.current.warn) } },
        colors = ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.background),
        modifier =
        Modifier
            .alpha(if (mission.ready) 1f else DIMMED)
            .clickable(enabled = mission.ready) { open(mission) },
    )
}

@Composable
private fun LinkRow(
    link: LinkState,
    onReconnect: () -> Unit,
) {
    val status = LocalStatusColors.current
    val (label, color) =
        when (link) {
            is LinkState.Online -> R.string.link_online to status.ok
            is LinkState.Connecting -> R.string.link_connecting to status.warn
            is LinkState.Refused -> R.string.link_refused to status.danger
            LinkState.Offline -> R.string.link_offline to status.danger
        }
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier =
        Modifier
            .clickable(enabled = link == LinkState.Offline, onClick = onReconnect)
            .padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        Dot(color)
        Text(stringResource(label), modifier = Modifier.padding(start = 8.dp))
    }
}

@Composable
private fun Dot(color: Color) {
    Box(Modifier.size(10.dp).background(color, CircleShape))
}

@Composable
private fun WorkspaceMenu(
    state: MissionsUiState,
    model: MissionsViewModel,
) {
    val view = state.view ?: return
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { open = true }) { Text(view.workspace.name, maxLines = 1) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            view.workspaces.filter { it.id != view.workspace.id }.forEach { ws ->
                DropdownMenuItem(text = { Text(ws.name) }, onClick = {
                    open = false
                    model.askWorkspace(ws)
                })
            }
        }
    }
}

fun kindLabel(kind: MissionKind): Int = when (kind) {
    MissionKind.HUNT -> R.string.kind_hunt
    MissionKind.DF_DRIVE -> R.string.kind_df
    MissionKind.RADAR_WATCH -> R.string.kind_radar
    MissionKind.SURVEY -> R.string.kind_survey
}

private const val DIMMED = 0.5f
