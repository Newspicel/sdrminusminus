package dev.newspicel.sdrmm.radar

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class RadarScreenTest {
    @get:Rule val compose = createComposeRule()

    private fun state(rows: List<RadarRow>) = RadarUiState(
        title = "FM",
        frame = null,
        frameVersion = 0,
        rangeMaxKm = 30f,
        dopplerSpanHz = 200f,
        rows = rows,
        echoes = 4u,
        stale = true,
        problems = emptyList(),
        imageFailed = false,
        link = LinkState.Online("Bench"),
    )

    @Test
    fun rows() {
        val row = RadarRow(7u, "T07", "12.4 km", "-35 Hz", R.string.radar_closing, null)
        compose.setContent { SdrmmTheme { RadarContent(state(listOf(row)), onBack = {}) } }
        compose.onNodeWithText("T07 · 12.4 km · -35 Hz · Closing").assertIsDisplayed()
        compose.onNodeWithText("Stale").assertIsDisplayed()
        compose.onNodeWithText("4 echoes").assertExists()
        compose.onNodeWithText("No image").assertIsDisplayed()
    }

    @Test
    fun empty() {
        compose.setContent { SdrmmTheme { RadarContent(state(emptyList()), onBack = {}) } }
        compose.onNodeWithText("No tracks").assertIsDisplayed()
    }
}
