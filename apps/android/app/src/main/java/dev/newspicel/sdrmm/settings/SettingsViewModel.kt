package dev.newspicel.sdrmm.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.ErrorText
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.AlignState
import dev.newspicel.sdrmm.ffi.CoreAbout
import dev.newspicel.sdrmm.ffi.HeadingMode
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.Mount
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.haptics.Haptics
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.sensors.SensorHub
import dev.newspicel.sdrmm.sensors.SensorStatus
import dev.newspicel.sdrmm.speech.Speaker
import dev.newspicel.sdrmm.speech.SpeakerState
import dev.newspicel.sdrmm.ui.AppNavigator
import dev.newspicel.sdrmm.ui.Destination
import dev.newspicel.sdrmm.ui.components.UiText
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class SettingsUiState(
    val settings: AppSettings,
    val servers: List<SavedServer>,
    val serversError: UiText?,
    val link: LinkState,
    val align: AlignState,
    val sensors: SensorStatus,
    val speaker: SpeakerState,
    val voices: List<String>,
    val vibrator: Boolean,
    val about: CoreAbout,
    val appVersion: String,
    val confirmForget: SavedServer?,
)

class SettingsViewModel(
    private val core: CoreGateway,
    private val settings: SettingsStore,
    sensors: SensorHub,
    private val navigator: AppNavigator,
    private val router: NoticeRouter,
    private val speaker: Speaker,
    haptics: Haptics,
    private val activate: (String) -> Unit,
    appVersion: String,
) : ViewModel() {
    private val servers = MutableStateFlow(loadServers())
    private val forget = MutableStateFlow<SavedServer?>(null)
    private val about = core.about()

    val state: StateFlow<SettingsUiState> =
        combine(settings.settings, servers, core.link, core.pose, sensors.status) { current, listed, link, pose, status ->
            Snapshot(current, listed, link, pose?.align ?: AlignState.Idle, status)
        }.combine(combine(forget, speaker.state, speaker.voices, ::Triple)) { snapshot, (asked, voice, voices) ->
            SettingsUiState(
                settings = snapshot.settings,
                servers = snapshot.listed.servers,
                serversError = snapshot.listed.error,
                link = snapshot.link,
                align = snapshot.align,
                sensors = snapshot.status,
                speaker = voice,
                voices = voices,
                vibrator = haptics.available,
                about = about,
                appVersion = appVersion,
                confirmForget = asked,
            )
        }.stateIn(
            viewModelScope,
            SharingStarted.Eagerly,
            SettingsUiState(
                settings = settings.settings.value,
                servers = servers.value.servers,
                serversError = servers.value.error,
                link = core.link.value,
                align = core.pose.value?.align ?: AlignState.Idle,
                sensors = sensors.status.value,
                speaker = speaker.state.value,
                voices = speaker.voices.value,
                vibrator = haptics.available,
                about = about,
                appVersion = appVersion,
                confirmForget = null,
            ),
        )

    fun connect(server: SavedServer) {
        activate(server.id)
    }

    fun askForget(server: SavedServer) {
        forget.value = server
    }

    fun dismissForget() {
        forget.value = null
    }

    fun confirmForget() {
        val server = forget.value ?: return
        forget.value = null
        when (val forgotten = core.forgetServer(server.id)) {
            is Outcome.Failed -> router.report(forgotten.error)
            is Outcome.Ok -> forgotten(server)
        }
    }

    private fun forgotten(server: SavedServer) {
        servers.value = loadServers()
        val wasActive = settings.settings.value.activeServerId == server.id
        if (wasActive) core.disconnect()
        viewModelScope.launch {
            if (wasActive) settings.update { it.copy(activeServerId = null) }
            if (wasActive || servers.value.servers.isEmpty()) navigator.toPair()
        }
    }

    fun addServer() {
        navigator.push(Destination.Pair)
    }

    fun openLicenses() {
        navigator.push(Destination.Licenses)
    }

    fun setName(name: String) = change { it.copy(phoneName = name.take(MAX_NAME)) }

    fun setSource(mode: HeadingMode) = change { it.copy(headingMode = mode) }

    fun setMount(mount: Mount) = change { it.copy(mount = mount) }

    fun stepOffset(delta: Int) = change { it.copy(mountOffsetDeg = (it.mountOffsetDeg + delta).coerceIn(-MAX_OFFSET, MAX_OFFSET)) }

    fun align(start: Boolean) {
        if (start) core.startAlign() else core.cancelAlign()
    }

    fun setUnits(units: Units) = change { it.copy(units = units) }

    fun setVoice(on: Boolean) = change { it.copy(voice = on) }

    fun pickVoice(name: String) = change { it.copy(voiceName = name) }

    fun testVoice(phrase: String) {
        speaker.say(phrase, urgent = true)
    }

    fun setNav(choice: NavChoice) = change { it.copy(navChoice = choice) }

    fun setKeepOn(on: Boolean) = change { it.copy(keepScreenOn = on) }

    fun setTiles(on: Boolean) = change { it.copy(mapTiles = on) }

    private fun change(transform: (AppSettings) -> AppSettings) {
        viewModelScope.launch { settings.update(transform) }
    }

    private fun loadServers(): Listed = when (val listed = core.savedServers()) {
        is Outcome.Ok -> Listed(listed.value, null)
        is Outcome.Failed -> Listed(emptyList(), ErrorText.short(listed.error))
    }

    private data class Listed(
        val servers: List<SavedServer>,
        val error: UiText?,
    )

    private data class Snapshot(
        val settings: AppSettings,
        val listed: Listed,
        val link: LinkState,
        val align: AlignState,
        val status: SensorStatus,
    )

    companion object {
        const val MAX_NAME = 64
        private const val MAX_OFFSET = 180.0
    }
}
