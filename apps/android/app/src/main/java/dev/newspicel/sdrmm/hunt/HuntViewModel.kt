package dev.newspicel.sdrmm.hunt

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.audio.ClickState
import dev.newspicel.sdrmm.audio.ClickTrack
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.HuntView
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.SweepPhase
import dev.newspicel.sdrmm.haptics.Haptics
import dev.newspicel.sdrmm.haptics.HuntHapticPolicy
import dev.newspicel.sdrmm.mission.BackgroundState
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.sensors.SensorHub
import dev.newspicel.sdrmm.sensors.SensorStatus
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.ui.components.Tuning
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

data class HuntUiState(
    val title: String,
    val view: HuntView?,
    val controls: List<MissionControl>,
    val clicks: Boolean,
    val clickState: ClickState,
    val haptics: Boolean,
    val busy: Boolean,
    val tuneOpen: Boolean,
    val link: LinkState,
    val sensors: SensorStatus,
    val pose: PoseView?,
    val background: BackgroundState,
) {
    val sweeping: Boolean get() = view?.sweep?.let { it.phase != SweepPhase.OFF } == true
}

class HuntViewModel(
    private val missionId: String,
    private val core: CoreGateway,
    private val settings: SettingsStore,
    sensors: SensorHub,
    runner: MissionRunner,
    clicks: ClickTrack,
    private val haptics: Haptics,
    private val router: NoticeRouter,
    private val clock: () -> Long = System::currentTimeMillis,
) : ViewModel() {
    private val local = MutableStateFlow(Local(busy = false, tuneOpen = false))
    private val policy = HuntHapticPolicy()
    private var resumed = false

    val state: StateFlow<HuntUiState> =
        combine(
            combine(core.hunt, core.missions, settings.settings, ::Triple),
            combine(clicks.state, local, ::Pair),
            combine(core.link, sensors.status, core.pose, ::Triple),
            runner.background,
        ) { (hunt, missions, current), (clickState, flags), (link, status, pose), background ->
            val mission = missions?.missions?.firstOrNull { it.id == missionId }
            HuntUiState(
                title = mission?.title ?: missionId,
                view = hunt?.takeIf { it.mission == missionId },
                controls = mission?.controls.orEmpty(),
                clicks = current.clicks,
                clickState = clickState,
                haptics = current.haptics,
                busy = flags.busy,
                tuneOpen = flags.tuneOpen,
                link = link,
                sensors = status,
                pose = pose,
                background = background,
            )
        }.stateIn(viewModelScope, SharingStarted.Eagerly, initial(sensors, runner, clicks))

    init {
        viewModelScope.launch { core.hunt.collect(::cue) }
    }

    private fun initial(
        sensors: SensorHub,
        runner: MissionRunner,
        clicks: ClickTrack,
    ): HuntUiState {
        val mission = core.missions.value?.missions?.firstOrNull { it.id == missionId }
        return HuntUiState(
            title = mission?.title ?: missionId,
            view = core.hunt.value?.takeIf { it.mission == missionId },
            controls = mission?.controls.orEmpty(),
            clicks = settings.settings.value.clicks,
            clickState = clicks.state.value,
            haptics = settings.settings.value.haptics,
            busy = false,
            tuneOpen = false,
            link = core.link.value,
            sensors = sensors.status.value,
            pose = core.pose.value,
            background = runner.background.value,
        )
    }

    private fun cue(view: HuntView?) {
        if (view == null || view.mission != missionId || !resumed || !settings.settings.value.haptics) return
        policy.cue(view.trend, view.strength, clock())?.let(haptics::play)
    }

    fun toggleRun() {
        send(if (state.value.view?.running == true) MissionCommand.StopHunt else MissionCommand.StartHunt)
    }

    fun toggleSweep() {
        send(MissionCommand.Sweep(!state.value.sweeping))
    }

    fun mark() {
        send(MissionCommand.Mark)
    }

    fun setClicks(on: Boolean) {
        viewModelScope.launch { settings.update { it.copy(clicks = on) } }
    }

    fun setHaptics(on: Boolean) {
        viewModelScope.launch { settings.update { it.copy(haptics = on) } }
    }

    fun openTune() {
        local.value = local.value.copy(tuneOpen = true)
    }

    fun closeTune() {
        local.value = local.value.copy(tuneOpen = false)
    }

    fun tune(megahertz: String): Int? {
        val hz = Tuning.hz(megahertz) ?: return R.string.bad_frequency
        closeTune()
        send(MissionCommand.Tune(hz))
        return null
    }

    fun onResumed(resumed: Boolean) {
        this.resumed = resumed
    }

    private fun send(command: MissionCommand) {
        viewModelScope.launch {
            local.value = local.value.copy(busy = true)
            val sent = core.send(command)
            local.value = local.value.copy(busy = false)
            if (sent is Outcome.Failed) router.report(sent.error)
        }
    }

    private data class Local(
        val busy: Boolean,
        val tuneOpen: Boolean,
    )
}
