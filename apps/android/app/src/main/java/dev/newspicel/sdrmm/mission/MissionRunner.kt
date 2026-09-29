package dev.newspicel.sdrmm.mission

import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

sealed interface BackgroundState {
    data object Idle : BackgroundState

    data object Running : BackgroundState

    data class Off(
        val reason: String,
    ) : BackgroundState
}

interface ServiceControl {
    fun start(): String?

    fun stop()
}

class MissionRunner(
    private val core: CoreGateway,
    private val service: ServiceControl,
) {
    private val current = MutableStateFlow<String?>(null)
    private val backgroundState = MutableStateFlow<BackgroundState>(BackgroundState.Idle)
    val open: StateFlow<String?> = current.asStateFlow()
    val background: StateFlow<BackgroundState> = backgroundState.asStateFlow()

    fun open(missionId: String): Outcome<Unit> {
        val opened = core.openMission(missionId)
        if (opened !is Outcome.Ok) return opened
        current.value = missionId
        if (backgroundState.value != BackgroundState.Running) {
            service.start()?.let(::serviceFailed)
        }
        return opened
    }

    fun close() {
        service.stop()
        core.closeMission()
        current.value = null
        backgroundState.value = BackgroundState.Idle
    }

    fun serviceStarted() {
        backgroundState.value = BackgroundState.Running
    }

    fun serviceFailed(reason: String) {
        backgroundState.value = BackgroundState.Off(reason)
    }

    fun serviceStopped() {
        if (backgroundState.value == BackgroundState.Running) backgroundState.value = BackgroundState.Idle
    }
}
