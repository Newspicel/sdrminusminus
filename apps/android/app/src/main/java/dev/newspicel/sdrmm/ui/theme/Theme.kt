package dev.newspicel.sdrmm.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf

val LocalStatusColors = staticCompositionLocalOf { DarkStatus }

@Composable
fun SdrmmTheme(
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    CompositionLocalProvider(LocalStatusColors provides if (dark) DarkStatus else LightStatus) {
        MaterialTheme(
            colorScheme = if (dark) DarkColors else LightColors,
            typography = SdrmmTypography,
            content = content,
        )
    }
}
