package dev.newspicel.sdrmm.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

val SdrmmTypography = Typography()

val Readout =
    TextStyle(
        fontFamily = FontFamily.Monospace,
        fontFeatureSettings = "tnum",
        fontWeight = FontWeight.Medium,
        fontSize = 16.sp,
    )
