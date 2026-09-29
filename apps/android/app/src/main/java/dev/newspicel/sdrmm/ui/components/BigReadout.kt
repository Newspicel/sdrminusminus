package dev.newspicel.sdrmm.ui.components

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import dev.newspicel.sdrmm.ui.theme.Readout

@Composable
fun BigReadout(
    text: String,
    modifier: Modifier = Modifier,
    size: TextUnit = 40.sp,
    color: Color = MaterialTheme.colorScheme.onBackground,
) {
    Text(text, style = Readout.copy(fontSize = size), color = color, maxLines = 1, modifier = modifier)
}
