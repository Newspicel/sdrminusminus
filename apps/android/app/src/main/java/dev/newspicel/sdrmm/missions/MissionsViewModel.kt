package dev.newspicel.sdrmm.missions

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.MissionsView
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.WorkspaceRef
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.sensors.SensorHub
import dev.newspicel.sdrmm.sensors.SensorStatus
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.ui.AppNavigator
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

data class MissionSection(
    val kind: MissionKind,
    val missions: List<Mission>,
)

data class MissionsUiState(
    val serverName: String,
    val link: LinkState,
    val view: MissionsView?,
    val sections: List<MissionSection>,
    val confirmWorkspace: WorkspaceRef?,
    val sensors: SensorStatus,
    val pose: PoseView?,
    val refreshing: Boolean,
)

class MissionsViewModel(
    private val core: CoreGateway,
    private val settings: SettingsStore,
    sensors: SensorHub,
    private val navigator: AppNavigator,
    private val router: NoticeRouter,
) : ViewModel() {
    private val asked = MutableStateFlow<WorkspaceRef?>(null)
    private val refreshing = MutableStateFlow(false)
    private val activeName =
        settings.settings
            .map { it.activeServerId }
            .distinctUntilChanged()
            .map(::savedName)

    val state: StateFlow<MissionsUiState> =
        combine(core.link, core.missions, sensors.status, core.pose, combine(asked, refreshing, activeName, ::Triple)) { link, view, status, pose, (ask, busy, saved) ->
            MissionsUiState(serverName(saved, link), link, view, sections(view), ask, status, pose, busy)
        }.stateIn(
            viewModelScope,
            SharingStarted.Eagerly,
            MissionsUiState(
                serverName(savedName(settings.settings.value.activeServerId), core.link.value),
                core.link.value,
                core.missions.value,
                sections(core.missions.value),
                null,
                sensors.status.value,
                core.pose.value,
                false,
            ),
        )

    fun open(mission: Mission) {
        if (!mission.ready) return
        navigator.openMission(mission)
    }

    fun refresh() {
        viewModelScope.launch {
            refreshing.value = true
            val refreshed = core.refreshMissions()
            refreshing.value = false
            if (refreshed is Outcome.Failed) router.report(refreshed.error)
        }
    }

    fun reconnect() {
        viewModelScope.launch {
            val id = settings.current().activeServerId ?: return@launch
            val connected = core.connect(id)
            if (connected is Outcome.Failed) router.report(connected.error)
        }
    }

    fun askWorkspace(ws: WorkspaceRef) {
        asked.value = ws
    }

    fun dismissWorkspace() {
        asked.value = null
    }

    fun confirmWorkspace() {
        val ws = asked.value ?: return
        asked.value = null
        viewModelScope.launch {
            val switched = core.switchWorkspace(ws.id)
            if (switched is Outcome.Failed) router.report(switched.error)
        }
    }

    private fun savedName(active: String?): String? = active?.let { id -> (core.savedServers() as? Outcome.Ok)?.value?.firstOrNull { it.id == id }?.name }

    private fun serverName(
        saved: String?,
        link: LinkState,
    ): String = saved ?: (link as? LinkState.Online)?.server.orEmpty()

    companion object {
        val ORDER = listOf(MissionKind.HUNT, MissionKind.DF_DRIVE, MissionKind.RADAR_WATCH, MissionKind.SURVEY)

        fun sections(view: MissionsView?): List<MissionSection> = ORDER.mapNotNull { kind ->
            view?.missions?.filter { it.kind == kind }?.takeIf { it.isNotEmpty() }?.let { MissionSection(kind, it) }
        }
    }
}
