package dev.newspicel.sdrmm.pair

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.testing.TestSdrmmApp
import dev.newspicel.sdrmm.ui.AppRoot
import dev.newspicel.sdrmm.ui.GraphRoot
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.annotation.Config

@RunWith(AndroidJUnit4::class)
@Config(application = TestSdrmmApp::class)
class PairScreenTest {
    @get:Rule val compose = createComposeRule()

    private val app: TestSdrmmApp get() = ApplicationProvider.getApplicationContext()

    @Test
    fun manual() {
        val graph = TestAppGraph.create(app)
        compose.setContent { SdrmmTheme { GraphRoot(graph) } }
        compose.onNode(hasText("Pair") and !hasSetTextAction() and hasClickAction()).assertIsNotEnabled()
        compose.onNode(hasSetTextAction() and hasText("host:port")).performTextInput("10.0.2.2:8443")
        compose.onNode(hasText("Pair") and !hasSetTextAction() and hasClickAction()).assertIsNotEnabled()
        compose.onNode(hasSetTextAction() and hasText("Code")).performTextInput("4821093")
        compose.onNode(hasText("Pair") and !hasSetTextAction() and hasClickAction()).assertIsNotEnabled()
        compose.onNode(hasSetTextAction() and hasText("Code")).performTextInput("7")
        compose.onNode(hasText("Pair") and !hasSetTextAction() and hasClickAction()).assertIsEnabled()
    }

    @Test
    fun demoOpensMissionsWithoutAServer() {
        compose.setContent { SdrmmTheme { AppRoot(app) } }
        compose.onNodeWithText("Try without a server").performScrollTo().performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Kraken DF").assertIsDisplayed()
        compose.onNodeWithText("Leave demo").assertIsDisplayed()
        compose.onNodeWithText("Leave demo").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Scan QR").assertIsDisplayed()
    }
}
