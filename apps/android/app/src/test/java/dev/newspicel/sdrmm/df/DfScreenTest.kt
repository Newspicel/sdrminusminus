package dev.newspicel.sdrmm.df

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.annotation.Config

@RunWith(AndroidJUnit4::class)
@Config(qualifiers = "w411dp-h891dp")
class DfScreenTest {
    @get:Rule(order = 0)
    val main = MainDispatcherRule()

    @get:Rule(order = 1)
    val compose = createComposeRule()

    private val core = FakeCoreGateway()
    private val settings = FakeSettingsStore(AppSettings(phoneName = "Pixel", units = Units.Metric))
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core, settings) }

    private fun show() {
        core.missions.value =
            Samples.missions(
                Samples.mission("d1", MissionKind.DF_DRIVE, "Kraken").copy(
                    controls = listOf(MissionControl.TARGET_MODE, MissionControl.CLEAR_FUSION, MissionControl.CALIBRATE),
                ),
            )
        val model = DfViewModel("d1", core, graph.settings, graph.sensors, graph.runner, graph.nav, graph.alerts, graph.router)
        compose.setContent {
            SdrmmTheme {
                val state by model.state.collectAsStateWithLifecycle()
                DfContent(state, model, onBack = {}, onChipAction = {}, onNavigate = {}, mapSlot = { Box(it.testTag(MAP)) })
            }
        }
    }

    @Test
    fun guidance() {
        core.df.value = Samples.df()
        show()
        compose.onNodeWithText("Cross 215° · 1.2 km").assertIsDisplayed()
        compose.onNodeWithTag(MAP).assertExists()
        core.df.value = Samples.df(guidance = null, state = DfState.CALIBRATING)
        compose.waitForIdle()
        compose.onNodeWithText("No guidance").assertIsDisplayed()
        compose.onNodeWithText("Calibrating").assertIsDisplayed()
    }

    @Test
    fun clear_dialog() {
        core.df.value = Samples.df()
        show()
        compose.onNodeWithText("Clear").performClick()
        compose.onNodeWithText("Clear fusion?").assertIsDisplayed()
    }

    private companion object {
        const val MAP = "map"
    }
}
