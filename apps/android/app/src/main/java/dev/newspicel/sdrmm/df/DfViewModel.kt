package dev.newspicel.sdrmm.df

import android.app.Activity
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.TargetMode
import dev.newspicel.sdrmm.map.CameraFollow
import dev.newspicel.sdrmm.mission.AlertNotifier
import dev.newspicel.sdrmm.mission.BackgroundState
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.nav.NavHandoff
import dev.newspicel.sdrmm.sensors.SensorHub
import dev.newspicel.sdrmm.sensors.SensorStatus
import dev.newspicel.sdrmm.settings.MapLayers
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.components.Tuning
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

enum class MapLayer { Rays, Heat, Ellipse }

data class DfUiState(
    val title: String,
    val view: DfView?,
    val rose: RoseModel,
    val controls: List<MissionControl>,
    val layers: MapLayers,
    val follow: CameraFollow,
    val confirmClear: Boolean,
    val tuneOpen: Boolean,
    val link: LinkState,
    val pose: PoseView?,
    val fix: LocationSample?,
    val sensors: SensorStatus,
    val background: BackgroundState,
    val alertsOff: Boolean,
    val tilesFailed: Boolean,
    val units: Units,
    val fitRequest: Int,
    val tiles: Boolean,
)

class DfViewModel(
    private val missionId: String,
    private val core: CoreGateway,
    private val settings: SettingsStore,
    private val sensors: SensorHub,
    private val runner: MissionRunner,
    private val nav: NavHandoff,
    private val alerts: AlertNotifier,
    private val router: NoticeRouter,
) : ViewModel() {
    private val local = MutableStateFlow(Local(alertsOff = !alerts.enabled))

    val state: StateFlow<DfUiState> =
        combine(
            combine(core.df, core.pose, sensors.lastFix, ::Triple),
            combine(core.missions, settings.settings, local, ::Triple),
            combine(core.link, sensors.status, runner.background, ::Triple),
        ) { (df, pose, fix), (missions, current, flags), (link, status, background) ->
            val mission = missions?.missions?.firstOrNull { it.id == missionId }
            val view = df?.takeIf { it.mission == missionId }
            DfUiState(
                title = mission?.title ?: missionId,
                view = view,
                rose = RoseGeometry.model(view, pose),
                controls = mission?.controls.orEmpty(),
                layers = current.layers,
                follow = flags.follow,
                confirmClear = flags.confirmClear,
                tuneOpen = flags.tuneOpen,
                link = link,
                pose = pose,
                fix = fix,
                sensors = status,
                background = background,
                alertsOff = flags.alertsOff,
                tilesFailed = flags.tilesFailed,
                units = Format.resolve(current.units),
                fitRequest = flags.fitRequest,
                tiles = current.mapTiles,
            )
        }.stateIn(viewModelScope, SharingStarted.Eagerly, initial())

    private fun initial(): DfUiState {
        val mission = core.missions.value?.missions?.firstOrNull { it.id == missionId }
        val view = core.df.value?.takeIf { it.mission == missionId }
        val current = settings.settings.value
        return DfUiState(
            title = mission?.title ?: missionId,
            view = view,
            rose = RoseGeometry.model(view, core.pose.value),
            controls = mission?.controls.orEmpty(),
            layers = current.layers,
            follow = local.value.follow,
            confirmClear = false,
            tuneOpen = false,
            link = core.link.value,
            pose = core.pose.value,
            fix = sensors.lastFix.value,
            sensors = sensors.status.value,
            background = runner.background.value,
            alertsOff = local.value.alertsOff,
            tilesFailed = false,
            units = Format.resolve(current.units),
            fitRequest = 0,
            tiles = current.mapTiles,
        )
    }

    fun setMode(mode: TargetMode) = send(MissionCommand.SetTargetMode(mode))

    fun navigate(activity: Activity) {
        val target = state.value.view?.target ?: return
        router.handoff(nav.open(activity, missionId, target.at))
    }

    fun calibrate() = send(MissionCommand.Calibrate)

    fun askClear() = local.update { it.copy(confirmClear = true) }

    fun dismissClear() = local.update { it.copy(confirmClear = false) }

    fun confirmClear() {
        dismissClear()
        send(MissionCommand.ClearFusion)
    }

    fun toggleLayer(layer: MapLayer) {
        viewModelScope.launch {
            settings.update { current ->
                val layers = current.layers
                current.copy(
                    layers =
                    when (layer) {
                        MapLayer.Rays -> layers.copy(rays = !layers.rays)
                        MapLayer.Heat -> layers.copy(heat = !layers.heat)
                        MapLayer.Ellipse -> layers.copy(ellipse = !layers.ellipse)
                    },
                )
            }
        }
    }

    fun cycleFollow() = local.update {
        it.copy(
            follow =
            when (it.follow) {
                CameraFollow.UserHeading -> CameraFollow.User
                CameraFollow.User, CameraFollow.Free -> CameraFollow.UserHeading
            },
        )
    }

    fun gesture() = local.update { it.copy(follow = CameraFollow.Free) }

    fun fit() = local.update { it.copy(follow = CameraFollow.Free, fitRequest = it.fitRequest + 1) }

    fun fitEmpty() = local.update { it.copy(follow = CameraFollow.User) }

    fun openTune() = local.update { it.copy(tuneOpen = true) }

    fun closeTune() = local.update { it.copy(tuneOpen = false) }

    fun tune(megahertz: String): Int? {
        val hz = Tuning.hz(megahertz) ?: return R.string.bad_frequency
        closeTune()
        send(MissionCommand.Tune(hz))
        return null
    }

    fun tilesFailed() = local.update { it.copy(tilesFailed = true) }

    fun permissionsChanged() = local.update { it.copy(alertsOff = !alerts.enabled) }

    private fun send(command: MissionCommand) {
        viewModelScope.launch {
            val sent = core.send(command)
            if (sent is Outcome.Failed) router.report(sent.error)
        }
    }

    private data class Local(
        val alertsOff: Boolean,
        val follow: CameraFollow = CameraFollow.UserHeading,
        val confirmClear: Boolean = false,
        val tuneOpen: Boolean = false,
        val tilesFailed: Boolean = false,
        val fitRequest: Int = 0,
    )
}
