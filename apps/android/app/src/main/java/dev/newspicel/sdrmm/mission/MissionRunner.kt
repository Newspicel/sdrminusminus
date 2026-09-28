package dev.newspicel.sdrmm.mission

import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

class MissionRunner(
    private val core: CoreGateway,
) {
    private val current = MutableStateFlow<String?>(null)
    val open: StateFlow<String?> = current.asStateFlow()

    fun open(missionId: String): Outcome<Unit> {
        val opened = core.openMission(missionId)
        if (opened is Outcome.Ok) current.value = missionId
        return opened
    }

    fun close() {
        core.closeMission()
        current.value = null
    }
}
