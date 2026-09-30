package dev.newspicel.sdrmm.audio

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.testing.FakeClickTrack
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeServiceControl
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.Samples
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ClickControllerTest {
    private val core = FakeCoreGateway()
    private val settings = FakeSettingsStore()
    private val runner = MissionRunner(core, FakeServiceControl())
    private val track = FakeClickTrack()
    private val router = NoticeRouter(core, settings, Samples.retargets())
    private val controller = ClickController(core, runner, settings, track, router)

    @Test
    fun plays_when_running() = runTest {
        controller.run(backgroundScope)
        runner.open("m1")
        core.hunt.value = Samples.hunt(running = true, strength = 0.7f)
        runCurrent()
        assertThat(track.state.value).isEqualTo(ClickState.Playing)
        assertThat(track.strengths.last()).isEqualTo(0.7f)
        settings.settings.value = settings.settings.value.copy(clicks = false)
        runCurrent()
        assertThat(track.state.value).isEqualTo(ClickState.Stopped)
        settings.settings.value = settings.settings.value.copy(clicks = true)
        runCurrent()
        assertThat(track.state.value).isEqualTo(ClickState.Playing)
        runner.close()
        runCurrent()
        assertThat(track.state.value).isEqualTo(ClickState.Stopped)
    }

    @Test
    fun a_stopped_hunt_is_silent() = runTest {
        controller.run(backgroundScope)
        runner.open("m1")
        core.hunt.value = Samples.hunt(running = false)
        runCurrent()
        assertThat(track.state.value).isEqualTo(ClickState.Stopped)
    }

    @Test
    fun a_failed_track_turns_clicks_off_with_a_banner() = runTest {
        controller.run(backgroundScope)
        runCurrent()
        track.state.value = ClickState.Failed(-3)
        runCurrent()
        assertThat(settings.settings.value.clicks).isFalse()
        assertThat(router.banners.value).hasSize(1)
    }
}
