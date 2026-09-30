package dev.newspicel.sdrmm.ui.theme

import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

@Immutable
data class StatusColors(
    val warn: Color,
    val ok: Color,
    val danger: Color,
)

val DarkStatus = StatusColors(warn = Color(0xFFE4B750), ok = Color(0xFF59D38C), danger = Color(0xFFF66D67))
val LightStatus = StatusColors(warn = Color(0xFFA96B00), ok = Color(0xFF167645), danger = Color(0xFFBA1E20))

val DarkColors =
    darkColorScheme(
        background = Color(0xFF0D0F10),
        surface = Color(0xFF151719),
        surfaceContainer = Color(0xFF151719),
        surfaceVariant = Color(0xFF202326),
        outline = Color(0xFF2A2E31),
        outlineVariant = Color(0xFF2A2E31),
        onBackground = Color(0xFFE5E8EB),
        onSurface = Color(0xFFE5E8EB),
        onSurfaceVariant = Color(0xFFA6ACAF),
        primary = Color(0xFF3ECCE2),
        onPrimary = Color(0xFF051115),
        secondary = Color(0xFF3ECCE2),
        onSecondary = Color(0xFF051115),
        secondaryContainer = Color(0xFF16383E),
        onSecondaryContainer = Color(0xFFE5E8EB),
        tertiary = Color(0xFF3ECCE2),
        onTertiary = Color(0xFF051115),
        error = Color(0xFFF66D67),
    )

val LightColors =
    lightColorScheme(
        background = Color(0xFFEEF0F2),
        surface = Color(0xFFFBFCFD),
        surfaceContainer = Color(0xFFFBFCFD),
        surfaceVariant = Color(0xFFE7EAEC),
        outline = Color(0xFFD4D8DB),
        outlineVariant = Color(0xFFD4D8DB),
        onBackground = Color(0xFF1A2024),
        onSurface = Color(0xFF1A2024),
        onSurfaceVariant = Color(0xFF4D5459),
        primary = Color(0xFF00769C),
        onPrimary = Color(0xFFF8FDFE),
        secondary = Color(0xFF00769C),
        onSecondary = Color(0xFFF8FDFE),
        secondaryContainer = Color(0xFFCDE6EE),
        onSecondaryContainer = Color(0xFF1A2024),
        tertiary = Color(0xFF00769C),
        onTertiary = Color(0xFFF8FDFE),
        error = Color(0xFFBA1E20),
    )
