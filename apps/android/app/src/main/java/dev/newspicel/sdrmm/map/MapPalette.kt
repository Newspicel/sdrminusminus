package dev.newspicel.sdrmm.map

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.toArgb
import dev.newspicel.sdrmm.ui.theme.DarkColors
import dev.newspicel.sdrmm.ui.theme.DarkStatus
import dev.newspicel.sdrmm.ui.theme.LightColors
import dev.newspicel.sdrmm.ui.theme.LightStatus

class MapPalette(
    val accent: Int,
    val warn: Int,
    val ok: Int,
    val heat: Int,
    val station: Int,
    val ink: Int,
    val background: Int,
    val survey: IntArray,
) {
    companion object {
        const val SURVEY_STEPS = 8

        fun of(dark: Boolean): MapPalette {
            val scheme = if (dark) DarkColors else LightColors
            val status = if (dark) DarkStatus else LightStatus
            return MapPalette(
                accent = scheme.primary.toArgb(),
                warn = status.warn.toArgb(),
                ok = status.ok.toArgb(),
                heat = status.danger.toArgb(),
                station = scheme.onSurfaceVariant.toArgb(),
                ink = scheme.onBackground.toArgb(),
                background = scheme.background.toArgb(),
                survey = surveySteps(status.ok, status.warn, status.danger),
            )
        }

        fun surveySteps(
            low: Color,
            mid: Color,
            high: Color,
        ): IntArray = IntArray(SURVEY_STEPS) { step ->
            val t = step / (SURVEY_STEPS - 1f)
            val color = if (t <= HALF) lerp(low, mid, t / HALF) else lerp(mid, high, (t - HALF) / HALF)
            color.toArgb()
        }

        private const val HALF = 0.5f
    }
}
