package dev.newspicel.sdrmm.settings

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.BuildConfig
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.AlignHint
import dev.newspicel.sdrmm.ffi.AlignState
import dev.newspicel.sdrmm.ffi.HeadingMode
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.Mount
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionGate
import dev.newspicel.sdrmm.permissions.rememberPermissionGate
import dev.newspicel.sdrmm.sensors.LocationAccess
import dev.newspicel.sdrmm.speech.SpeakerState
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.components.BOTTOM_ROOM
import dev.newspicel.sdrmm.ui.components.ConfirmDialog
import dev.newspicel.sdrmm.ui.components.SectionTitle
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.components.UiText
import dev.newspicel.sdrmm.ui.components.text
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import dev.newspicel.sdrmm.ui.theme.Readout

@Composable
fun SettingsScreen(graph: AppGraph) {
    val model =
        viewModel {
            SettingsViewModel(
                graph.core,
                graph.settings,
                graph.sensors,
                graph.navigator,
                graph.router,
                graph.speaker,
                graph.haptics,
                graph::activate,
                BuildConfig.VERSION_NAME,
            )
        }
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val gate = rememberPermissionGate { _, _ -> graph.permissionsChanged() }
    SettingsContent(
        state = state,
        model = model,
        onBack = graph.navigator::back,
        allowLocation = { gate.request(Need.Location) },
        openLocationSettings = { PermissionGate.openAppSettings(context) },
        openSource = { openSource(context, graph) },
    )
}

@Composable
fun SettingsContent(
    state: SettingsUiState,
    model: SettingsViewModel,
    onBack: () -> Unit,
    allowLocation: () -> Unit,
    openLocationSettings: () -> Unit,
    openSource: () -> Unit,
) {
    Column(Modifier.safeDrawingPadding().verticalScroll(rememberScrollState())) {
        TopBar(stringResource(R.string.settings_title), onBack = onBack)
        ServersSection(state, model)
        PhoneSection(state, model)
        HeadingSection(state, model)
        UnitsSection(state, model)
        VoiceSection(state, model)
        NavigationSection(state, model)
        SectionTitle(stringResource(R.string.settings_display))
        ToggleRow(stringResource(R.string.keep_screen_on), state.settings.keepScreenOn, model::setKeepOn)
        ToggleRow(stringResource(R.string.map_tiles), state.settings.mapTiles, model::setTiles)
        LocationSection(state, allowLocation, openLocationSettings)
        AboutSection(state, model::openLicenses, openSource)
        Spacer(Modifier.height(BOTTOM_ROOM))
    }
    state.confirmForget?.let {
        ConfirmDialog(
            title = stringResource(R.string.forget_server),
            confirm = stringResource(R.string.forget),
            onConfirm = model::confirmForget,
            onDismiss = model::dismissForget,
        )
    }
}

@Composable
private fun ServersSection(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    SectionTitle(stringResource(R.string.settings_servers))
    state.serversError?.let { Text(it.text(), color = LocalStatusColors.current.danger, modifier = Modifier.padding(horizontal = 16.dp)) }
    state.servers.forEach { server ->
        ListItem(
            headlineContent = { Text(server.name) },
            supportingContent = { Text(linkLabel(server, state)) },
            trailingContent = { TextButton(onClick = { model.askForget(server) }) { Text(stringResource(R.string.forget)) } },
            colors = ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.background),
            modifier = Modifier.clickable { model.connect(server) },
        )
    }
    TextButton(onClick = model::addServer, modifier = Modifier.padding(horizontal = 8.dp)) {
        Text(stringResource(R.string.settings_add_server))
    }
}

@Composable
private fun linkLabel(
    server: SavedServer,
    state: SettingsUiState,
): String {
    if (state.settings.activeServerId != server.id) return stringResource(R.string.link_offline)
    return when (state.link) {
        is LinkState.Online -> stringResource(R.string.link_online)
        is LinkState.Connecting -> stringResource(R.string.link_connecting)
        is LinkState.Refused -> stringResource(R.string.link_refused)
        LinkState.Offline -> stringResource(R.string.link_offline)
    }
}

@Composable
private fun PhoneSection(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    SectionTitle(stringResource(R.string.settings_phone))
    var name by rememberSaveable { mutableStateOf(state.settings.phoneName) }
    OutlinedTextField(
        value = name,
        onValueChange = {
            name = it.take(SettingsViewModel.MAX_NAME)
            model.setName(name)
        },
        label = { Text(stringResource(R.string.settings_name)) },
        singleLine = true,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
    )
}

@Composable
private fun HeadingSection(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    SectionTitle(stringResource(R.string.settings_heading))
    Choices(
        label = stringResource(R.string.settings_source),
        options =
        listOf(
            HeadingMode.AUTO to stringResource(R.string.heading_auto),
            HeadingMode.COMPASS to stringResource(R.string.source_compass),
            HeadingMode.COURSE to stringResource(R.string.source_course),
        ),
        selected = state.settings.headingMode,
        onSelect = model::setSource,
    )
    Choices(
        label = stringResource(R.string.settings_mount),
        options = listOf(Mount.FLAT to stringResource(R.string.mount_flat), Mount.UPRIGHT to stringResource(R.string.mount_upright)),
        selected = state.settings.mount,
        onSelect = model::setMount,
    )
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 16.dp)) {
        Text(stringResource(R.string.settings_offset), modifier = Modifier.weight(1f))
        TextButton(onClick = { model.stepOffset(-1) }) { Text("-") }
        Text(Format.signedDegrees(state.settings.mountOffsetDeg), style = Readout)
        TextButton(onClick = { model.stepOffset(1) }) { Text("+") }
    }
    val collecting = state.align is AlignState.Collecting
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.padding(horizontal = 16.dp),
    ) {
        OutlinedButton(onClick = { model.align(!collecting) }) {
            Text(stringResource(if (collecting) R.string.cancel else R.string.align))
        }
        alignLabel(state.align)?.let { Text(it) }
    }
}

@Composable
private fun alignLabel(align: AlignState): String? = when (align) {
    AlignState.Idle -> null

    is AlignState.Collecting ->
        when (align.hint) {
            AlignHint.DRIVE_FASTER -> stringResource(R.string.align_faster)
            AlignHint.DRIVE_STRAIGHT -> stringResource(R.string.align_straight)
            AlignHint.HOLD -> stringResource(R.string.align_hold)
        }

    is AlignState.Done -> stringResource(R.string.align_done, Format.signedDegrees(align.offsetDeg))

    is AlignState.Failed -> align.reason
}

@Composable
private fun UnitsSection(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    SectionTitle(stringResource(R.string.settings_units))
    Choices(
        label = null,
        options =
        listOf(
            Units.Auto to stringResource(R.string.heading_auto),
            Units.Metric to stringResource(R.string.units_metric),
            Units.Imperial to stringResource(R.string.units_imperial),
        ),
        selected = state.settings.units,
        onSelect = model::setUnits,
    )
}

@Composable
private fun VoiceSection(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    SectionTitle(stringResource(R.string.settings_voice))
    ToggleRow(stringResource(R.string.settings_voice), state.settings.voice, model::setVoice)
    val failed = state.speaker as? SpeakerState.Failed
    if (failed != null) {
        Text(
            stringResource(R.string.no_voice),
            color = LocalStatusColors.current.danger,
            modifier = Modifier.padding(horizontal = 16.dp).semantics { contentDescription = failed.reason },
        )
        return
    }
    if (state.voices.isNotEmpty()) VoicePicker(state, model)
    val phrase = stringResource(R.string.voice_test_phrase)
    TextButton(
        onClick = { model.testVoice(phrase) },
        enabled = state.speaker == SpeakerState.Ready,
        modifier = Modifier.padding(horizontal = 8.dp),
    ) { Text(stringResource(R.string.voice_test)) }
}

@Composable
private fun VoicePicker(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    var open by remember { mutableStateOf(false) }
    Box(Modifier.padding(horizontal = 8.dp)) {
        TextButton(onClick = { open = true }) { Text(state.settings.voiceName ?: stringResource(R.string.heading_auto)) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            state.voices.forEach { name ->
                DropdownMenuItem(text = { Text(name) }, onClick = {
                    open = false
                    model.pickVoice(name)
                })
            }
        }
    }
}

@Composable
private fun NavigationSection(
    state: SettingsUiState,
    model: SettingsViewModel,
) {
    SectionTitle(stringResource(R.string.settings_navigation))
    Choices(
        label = null,
        options = listOf(NavChoice.GoogleMaps to stringResource(R.string.nav_google_maps), NavChoice.Ask to stringResource(R.string.nav_ask)),
        selected = state.settings.navChoice,
        onSelect = model::setNav,
    )
}

@Composable
private fun LocationSection(
    state: SettingsUiState,
    allowLocation: () -> Unit,
    openLocationSettings: () -> Unit,
) {
    SectionTitle(stringResource(R.string.settings_location))
    val on = state.sensors.access == LocationAccess.WhileUsing
    ValueRow(
        stringResource(R.string.location_access),
        stringResource(if (on) R.string.access_while_using else R.string.access_off),
        mono = false,
    ) {
        TextButton(onClick = if (on) openLocationSettings else allowLocation) { Text(stringResource(R.string.open_settings)) }
    }
    ValueRow(
        stringResource(R.string.location_precise),
        stringResource(if (state.sensors.precise) R.string.on else R.string.off),
        mono = false,
    ) {
        if (!state.sensors.precise) TextButton(onClick = allowLocation) { Text(stringResource(R.string.turn_on)) }
    }
    state.sensors.lastError?.let { Text(it, color = LocalStatusColors.current.danger, modifier = Modifier.padding(horizontal = 16.dp)) }
}

@Composable
private fun AboutSection(
    state: SettingsUiState,
    openLicenses: () -> Unit,
    openSource: () -> Unit,
) {
    SectionTitle(stringResource(R.string.settings_about))
    ValueRow(stringResource(R.string.version), state.appVersion)
    ValueRow(stringResource(R.string.core), state.about.coreVersion)
    ValueRow(stringResource(R.string.protocol), state.about.protocol.toString())
    TextButton(onClick = openLicenses, modifier = Modifier.padding(horizontal = 8.dp)) { Text(stringResource(R.string.licenses)) }
    TextButton(onClick = openSource, modifier = Modifier.padding(horizontal = 8.dp)) { Text(stringResource(R.string.source_code)) }
    if (!state.vibrator) Text(stringResource(R.string.no_vibrator), color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp))
}

@Composable
private fun <T> Choices(
    label: String?,
    options: List<Pair<T, String>>,
    selected: T?,
    onSelect: (T) -> Unit,
) {
    Column(Modifier.padding(horizontal = 16.dp)) {
        label?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            options.forEach { (value, text) ->
                FilterChip(selected = value == selected, onClick = { onSelect(value) }, label = { Text(text) })
            }
        }
    }
}

@Composable
private fun ToggleRow(
    label: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().clickable { onChange(!checked) }.padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

@Composable
private fun ValueRow(
    label: String,
    value: String,
    mono: Boolean = true,
    trailing: @Composable () -> Unit = {},
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        Text(label, modifier = Modifier.weight(1f))
        Text(
            value,
            style = if (mono) Readout else MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        trailing()
    }
}

const val SOURCE_URL = "https://github.com/newspicel/sdrminusminus"

private fun openSource(
    context: Context,
    graph: AppGraph,
) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, SOURCE_URL.toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
        graph.router.show(NoticeLevel.ERROR, UiText.Res(R.string.no_browser), SOURCE_URL)
    }
}
