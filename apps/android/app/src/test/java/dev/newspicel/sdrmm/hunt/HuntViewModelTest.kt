package dev.newspicel.sdrmm.hunt

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.SweepPhase
import dev.newspicel.sdrmm.ffi.Trend
import dev.newspicel.sdrmm.haptics.HapticCue
import dev.newspicel.sdrmm.mission.Banner
import dev.newspicel.sdrmm.testing.FakeClickTrack
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeHaptics
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.components.UiText
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class HuntViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway()
    private val haptics = FakeHaptics()
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core) }
    private var now = 0L
    private val model by lazy {
        core.missions.value = Samples.missions(Samples.mission("m1").copy(controls = listOf(MissionControl.HUNT_RUN, MissionControl.TUNE, MissionControl.SWEEP, MissionControl.MARK)))
        HuntViewModel("m1", core, graph.settings, graph.sensors, graph.runner, FakeClickTrack(), haptics, graph.router) { now }
    }

    @Test
    fun start_stop() {
        core.hunt.value = Samples.hunt(running = false)
        model.toggleRun()
        core.hunt.value = Samples.hunt(running = true)
        model.toggleRun()
        assertThat(core.commands).containsExactly(MissionCommand.StartHunt, MissionCommand.StopHunt).inOrder()
    }

    @Test
    fun tune_parse() {
        model.openTune()
        assertThat(model.state.value.tuneOpen).isTrue()
        assertThat(model.tune("abc")).isEqualTo(R.string.bad_frequency)
        assertThat(model.tune("0")).isEqualTo(R.string.bad_frequency)
        assertThat(model.tune("145,5")).isNull()
        assertThat(model.state.value.tuneOpen).isFalse()
        assertThat(core.commands.single()).isEqualTo(MissionCommand.Tune(145_500_000.0))
    }

    @Test
    fun failure_reported() {
        core.outcomes["send"] = Outcome.Failed(CoreException.NotConnected())
        model.toggleRun()
        val banner = graph.router.banners.value.single() as Banner.Text
        assertThat(banner.text).isEqualTo(UiText.Res(R.string.err_offline))
        assertThat(model.state.value.busy).isFalse()
    }

    @Test
    fun sweep_toggles_and_marks() {
        core.hunt.value = Samples.hunt(running = true)
        model.toggleSweep()
        core.hunt.value = Samples.hunt(running = true, sweep = Samples.sweep(phase = SweepPhase.SWEEPING))
        assertThat(model.state.value.sweeping).isTrue()
        model.toggleSweep()
        model.mark()
        assertThat(core.commands).containsExactly(MissionCommand.Sweep(true), MissionCommand.Sweep(false), MissionCommand.Mark).inOrder()
    }

    @Test
    fun haptics_only_while_resumed() {
        core.hunt.value = Samples.hunt(trend = Trend.COLDER)
        model.state.value
        core.hunt.value = Samples.hunt(trend = Trend.WARMER)
        assertThat(haptics.played).isEmpty()
        model.onResumed(true)
        now = 1_000
        core.hunt.value = Samples.hunt(trend = Trend.COLDER)
        assertThat(haptics.played).containsExactly(HapticCue.Decrease)
    }

    @Test
    fun other_missions_are_ignored() {
        core.hunt.value = Samples.hunt(mission = "other")
        assertThat(model.state.value.view).isNull()
        assertThat(model.state.value.title).isEqualTo("Beacon")
    }
}
