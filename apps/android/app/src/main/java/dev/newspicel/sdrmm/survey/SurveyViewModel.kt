package dev.newspicel.sdrmm.survey

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.SurveyView
import dev.newspicel.sdrmm.map.TrailRun
import dev.newspicel.sdrmm.mission.BackgroundState
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.sensors.SensorHub
import dev.newspicel.sdrmm.sensors.SensorStatus
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class SurveyUiState(
    val title: String,
    val view: SurveyView?,
    val runs: List<TrailRun>,
    val trimmed: Boolean,
    val controls: List<MissionControl>,
    val link: LinkState,
    val sensors: SensorStatus,
    val pose: PoseView?,
    val fix: LocationSample?,
    val background: BackgroundState,
    val fitRequest: Int,
    val tilesFailed: Boolean,
)

class SurveyViewModel(
    private val missionId: String,
    private val core: CoreGateway,
    private val sensors: SensorHub,
    private val runner: MissionRunner,
    private val trail: SurveyTrailStore,
    private val router: NoticeRouter,
) : ViewModel() {
    private val local = MutableStateFlow(Local())

    val state: StateFlow<SurveyUiState> =
        combine(
            combine(core.survey, core.missions, local, ::Triple),
            combine(trail.runs, trail.trimmed, ::Pair),
            combine(core.link, sensors.status, core.pose, ::Triple),
            combine(sensors.lastFix, runner.background, ::Pair),
        ) { (survey, missions, flags), (runs, trimmed), (link, status, pose), (fix, background) ->
            val mission = missions?.missions?.firstOrNull { it.id == missionId }
            SurveyUiState(
                title = mission?.title ?: missionId,
                view = survey?.takeIf { it.mission == missionId },
                runs = runs,
                trimmed = trimmed,
                controls = mission?.controls.orEmpty(),
                link = link,
                sensors = status,
                pose = pose,
                fix = fix,
                background = background,
                fitRequest = flags.fitRequest,
                tilesFailed = flags.tilesFailed,
            )
        }.stateIn(viewModelScope, SharingStarted.Eagerly, initial())

    private fun initial(): SurveyUiState = SurveyUiState(
        title = core.missions.value?.missions?.firstOrNull { it.id == missionId }?.title ?: missionId,
        view = core.survey.value?.takeIf { it.mission == missionId },
        runs = trail.runs.value,
        trimmed = trail.trimmed.value,
        controls = core.missions.value?.missions?.firstOrNull { it.id == missionId }?.controls.orEmpty(),
        link = core.link.value,
        sensors = sensors.status.value,
        pose = core.pose.value,
        fix = sensors.lastFix.value,
        background = runner.background.value,
        fitRequest = 0,
        tilesFailed = false,
    )

    fun clearTrail() {
        viewModelScope.launch { trail.clear() }
    }

    fun toggleRecord() {
        val command = if (state.value.view?.recording == true) MissionCommand.StopSurvey else MissionCommand.StartSurvey
        viewModelScope.launch {
            val sent = core.send(command)
            if (sent is Outcome.Failed) router.report(sent.error)
        }
    }

    fun fit() = local.update { it.copy(fitRequest = it.fitRequest + 1) }

    fun tilesFailed() = local.update { it.copy(tilesFailed = true) }

    fun extent(): List<LatLon> = buildList {
        state.value.runs.forEach { addAll(it.coordinates) }
        state.value.fix?.let { add(LatLon(it.lat, it.lon)) }
    }

    private data class Local(
        val fitRequest: Int = 0,
        val tilesFailed: Boolean = false,
    )
}
