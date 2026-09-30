package dev.newspicel.sdrmm.settings

import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.AlignState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

fun syncPoseSettings(
    scope: CoroutineScope,
    settings: SettingsStore,
    core: CoreGateway,
): Job = scope.launch {
    launch {
        settings.settings
            .map { it.poseSettings() }
            .distinctUntilChanged()
            .collect { core.setPoseSettings(it) }
    }
    launch {
        core.pose
            .map { it?.align }
            .distinctUntilChanged()
            .filterIsInstance<AlignState.Done>()
            .collect { done -> settings.update { it.copy(mountOffsetDeg = done.offsetDeg) } }
    }
}
