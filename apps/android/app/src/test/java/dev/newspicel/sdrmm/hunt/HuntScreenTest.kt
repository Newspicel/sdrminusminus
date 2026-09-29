package dev.newspicel.sdrmm.hunt

import androidx.compose.runtime.getValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.SweepPhase
import dev.newspicel.sdrmm.testing.FakeClickTrack
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeHaptics
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class HuntScreenTest {
    @get:Rule val compose = createComposeRule()

    private val core = FakeCoreGateway()
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core) }

    private fun model(vararg controls: MissionControl): HuntViewModel {
        core.missions.value = Samples.missions(Samples.mission("m1").copy(controls = controls.toList()))
        return HuntViewModel("m1", core, graph.settings, graph.sensors, graph.runner, FakeClickTrack(), FakeHaptics(), graph.router)
    }

    private fun show(model: HuntViewModel) {
        compose.setContent {
            SdrmmTheme {
                val state by model.state.collectAsStateWithLifecycle()
                HuntContent(state, model, onBack = {}, onChipAction = {})
            }
        }
    }

    @Test
    fun labels() {
        core.hunt.value = Samples.hunt()
        val model = model(MissionControl.HUNT_RUN, MissionControl.TUNE)
        show(model)
        compose.onNodeWithText("145.500 MHz").assertIsDisplayed()
        compose.onNodeWithText("Warmer").assertIsDisplayed()
        compose.onNodeWithText("-61.0 dB").assertIsDisplayed()
        compose.onNodeWithText("Start").performScrollTo().performClick()
        compose.waitForIdle()
        assertThat(core.commands).containsExactly(MissionCommand.StartHunt)
    }

    @Test
    fun sweep_section_shows_the_bearing() {
        core.hunt.value = Samples.hunt(running = true, sweep = Samples.sweep(phase = SweepPhase.SWEEPING))
        val model = model(MissionControl.HUNT_RUN, MissionControl.SWEEP, MissionControl.MARK)
        show(model)
        compose.onNodeWithText("Turn slowly").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("137°").assertIsDisplayed()
        compose.onNodeWithText("Mark").performScrollTo().performClick()
        compose.onNodeWithText("Sweep").performScrollTo().performClick()
        compose.waitForIdle()
        assertThat(core.commands).containsExactly(MissionCommand.Mark, MissionCommand.Sweep(false)).inOrder()
    }

    @Test
    fun no_sweep_without_the_control() {
        core.hunt.value = Samples.hunt()
        val model = model(MissionControl.HUNT_RUN)
        show(model)
        compose.onNodeWithText("Sweep").assertDoesNotExist()
    }
}
