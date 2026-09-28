package dev.newspicel.sdrmm.pair

import android.Manifest
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.testing.TestSdrmmApp
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PairFlowDeviceTest {
    @get:Rule(order = 0)
    val permissions: GrantPermissionRule =
        GrantPermissionRule.grant(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)

    @get:Rule(order = 1)
    val compose = createAndroidComposeRule<MainActivity>()

    @Test
    fun manual_pair_trusts_and_opens_missions() {
        val app = compose.activity.application as TestSdrmmApp
        compose.onNode(hasSetTextAction() and hasText("host:port")).performTextInput("10.0.2.2:8443")
        compose.onNode(hasSetTextAction() and hasText("Code")).performTextInput("4821 0937")
        compose.onNode(hasText("Pair") and hasClickAction() and !hasSetTextAction()).performClick()
        compose.onNodeWithText("Trust server?").assertExists()
        compose.onNodeWithText("Trust").performClick()
        compose.waitUntil(PATIENCE_MS) { compose.onAllNodesWithTextExists("Missions") }
        assertThat(app.fakeCore.calls).containsAtLeast("offerManual:10.0.2.2:8443", "pair:${app.fakeSettings.settings.value.phoneName}", "connect:s1")
        assertThat(app.fakeSettings.settings.value.activeServerId).isEqualTo("s1")
    }

    private fun androidx.compose.ui.test.junit4.ComposeTestRule.onAllNodesWithTextExists(text: String): Boolean = onAllNodes(hasText(text)).fetchSemanticsNodes().isNotEmpty()

    private companion object {
        const val PATIENCE_MS = 5_000L
    }
}
