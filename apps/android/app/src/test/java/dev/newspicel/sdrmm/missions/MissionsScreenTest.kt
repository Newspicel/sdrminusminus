package dev.newspicel.sdrmm.missions

import androidx.compose.runtime.getValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MissionsScreenTest {
    @get:Rule val compose = createComposeRule()

    private val core = FakeCoreGateway().apply { servers += Samples.server() }
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core) }

    private fun show(model: MissionsViewModel) {
        compose.setContent {
            SdrmmTheme {
                val state by model.state.collectAsStateWithLifecycle()
                MissionsContent(state, model, onSettings = {}, onChipAction = {})
            }
        }
    }

    @Test
    fun sections_and_empty() {
        val model = MissionsViewModel(core, graph.settings, graph.sensors, graph.navigator, graph.router)
        show(model)
        compose.onNodeWithText("No missions").assertIsDisplayed()
    }

    @Test
    fun sections_render_and_rows_open() {
        core.link.value = LinkState.Online("Bench")
        core.missions.value =
            Samples.missions(
                Samples.mission("h", MissionKind.HUNT, "Beacon"),
                Samples.mission("d", MissionKind.DF_DRIVE, "Kraken").copy(ready = false, blocker = "No array"),
            )
        val model = MissionsViewModel(core, graph.settings, graph.sensors, graph.navigator, graph.router)
        show(model)
        compose.onNodeWithText("Hunt").assertIsDisplayed()
        compose.onNodeWithText("DF drive").assertIsDisplayed()
        compose.onNodeWithText("No array").assertIsDisplayed()
        compose.onNodeWithText("Online").assertIsDisplayed()
        compose.onNodeWithText("Beacon").performClick()
        assertThat(core.calls).contains("openMission:h")
    }
}
