package dev.newspicel.sdrmm.settings

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SettingsScreenTest {
    @get:Rule val compose = createComposeRule()

    private val core = FakeCoreGateway().apply { servers += Samples.server() }

    @Test
    fun forget_dialog() {
        val graph = TestAppGraph.create(ApplicationProvider.getApplicationContext(), core)
        compose.setContent { SdrmmTheme { SettingsScreen(graph) } }
        compose.onNodeWithText("Bench").assertIsDisplayed()
        compose.onNodeWithText("Forget").performClick()
        compose.onNodeWithText("Forget server?").assertIsDisplayed()
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithText("Forget server?").assertDoesNotExist()
        assertThat(core.calls.none { it.startsWith("forgetServer") }).isTrue()
    }

    @Test
    fun about_shows_core_version() {
        val graph = TestAppGraph.create(ApplicationProvider.getApplicationContext(), core)
        compose.setContent { SdrmmTheme { SettingsScreen(graph) } }
        compose.onNodeWithText("0.0.0-test").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Licenses").performScrollTo().performClick()
        assertThat(graph.navigator.top()).isEqualTo(dev.newspicel.sdrmm.ui.Destination.Licenses)
    }

    @Test
    fun licenses_list_core_entries_and_android_notices() {
        compose.setContent {
            SdrmmTheme {
                LicensesContent(listOf(Samples.license()), AndroidNotices.Text(listOf("ZXing 3.5.4")), onBack = {})
            }
        }
        compose.onNodeWithText("uniffi 0.32.2").assertIsDisplayed().performClick()
        compose.onNodeWithText("Mozilla Public License 2.0").assertIsDisplayed()
        compose.onNodeWithText("ZXing 3.5.4").assertIsDisplayed()
    }
}
