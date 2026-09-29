package dev.newspicel.sdrmm.audio

import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.ui.components.UiText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

class ClickController(
    private val core: CoreGateway,
    private val runner: MissionRunner,
    private val settings: SettingsStore,
    private val track: ClickTrack,
    private val router: NoticeRouter,
) {
    fun run(scope: CoroutineScope): Job = scope.launch {
        launch {
            combine(core.hunt, runner.open, settings.settings.map { it.clicks }.distinctUntilChanged()) { hunt, open, clicks ->
                hunt?.let { if (it.running && clicks && open == it.mission) it.strength else null }
            }.collect { strength ->
                if (strength == null) {
                    track.stop()
                } else {
                    track.setStrength(strength)
                    track.start()
                }
            }
        }
        launch {
            track.state.collect { state ->
                if (state is ClickState.Failed) {
                    track.stop()
                    router.show(NoticeLevel.WARN, UiText.Res(R.string.audio_off), "AudioTrack ${state.code}")
                    settings.update { it.copy(clicks = false) }
                }
            }
        }
    }
}
