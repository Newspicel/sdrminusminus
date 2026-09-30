package dev.newspicel.sdrmm.df

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import kotlin.math.abs
import kotlin.math.roundToInt

@Composable
fun BearingRose(
    model: RoseModel,
    modifier: Modifier = Modifier,
) {
    val ring = MaterialTheme.colorScheme.outline
    val ink = MaterialTheme.colorScheme.onBackground
    val dim = MaterialTheme.colorScheme.onSurfaceVariant
    val accent = MaterialTheme.colorScheme.primary
    val warn = LocalStatusColors.current.warn
    val paint = remember { letterPaint() }
    Canvas(
        modifier.aspectRatio(1f).semantics {
            contentDescription = "Bearing ${Format.angle(model.bearingDeg)}"
            model.bearingDeg?.takeIf { model.headingUp }?.let { stateDescription = sideText(it) }
        },
    ) {
        val radius = size.minDimension / 2 * RING
        drawCircle(ring, radius, center, style = Stroke(1.5.dp.toPx()))
        ticks(model.northDeg, radius, dim)
        letters(model.northDeg, radius, ink, paint)
        if (model.headingUp) car(radius, dim)
        val bearing = model.bearingDeg
        if (bearing != null) {
            model.sigmaDeg?.takeIf { it.isFinite() && it > 0 }?.let { sigma ->
                drawArc(
                    color = accent.copy(alpha = WEDGE_ALPHA),
                    startAngle = (bearing - sigma - QUARTER).toFloat(),
                    sweepAngle = (2 * sigma).toFloat(),
                    useCenter = true,
                    topLeft = Offset(center.x - radius, center.y - radius),
                    size = Size(radius * 2, radius * 2),
                )
            }
            drawLine(accent, center, RoseGeometry.point(bearing, radius * NEEDLE, center.x, center.y), 3.dp.toPx(), cap = StrokeCap.Round)
        }
        model.guidanceDeg?.let {
            drawLine(warn, center, RoseGeometry.point(it, radius * GUIDANCE, center.x, center.y), 6.dp.toPx(), cap = StrokeCap.Round)
        }
        drawCircle(ink, 3.dp.toPx(), center)
    }
}

private fun DrawScope.ticks(
    northDeg: Double,
    radius: Float,
    color: Color,
) {
    for (step in 0 until TICKS) {
        val angle = northDeg + step * TICK_DEG
        val cardinal = step % CARDINAL_EVERY == 0
        val inner = radius * if (cardinal) LONG_TICK else SHORT_TICK
        drawLine(
            color,
            RoseGeometry.point(angle, inner, center.x, center.y),
            RoseGeometry.point(angle, radius, center.x, center.y),
            if (cardinal) 2.dp.toPx() else 1.dp.toPx(),
        )
    }
}

private fun letterPaint(): android.graphics.Paint = android.graphics.Paint().apply {
    textAlign = android.graphics.Paint.Align.CENTER
    isAntiAlias = true
}

private fun DrawScope.letters(
    northDeg: Double,
    radius: Float,
    color: Color,
    paint: android.graphics.Paint,
) {
    paint.color = color.toArgb()
    paint.textSize = 14.dp.toPx()
    CARDINALS.forEachIndexed { index, letter ->
        val at = RoseGeometry.point(northDeg + index * QUARTER, radius * LETTERS, center.x, center.y)
        drawContext.canvas.nativeCanvas.drawText(letter, at.x, at.y + paint.textSize / 3, paint)
    }
}

private fun DrawScope.car(
    radius: Float,
    color: Color,
) {
    val top = center.y - radius * CAR_AT
    val half = 6.dp.toPx()
    val path =
        Path().apply {
            moveTo(center.x, top - half)
            lineTo(center.x + half, top + half)
            lineTo(center.x - half, top + half)
            close()
        }
    drawPath(path, color)
}

fun sideText(relativeDeg: Double): String {
    val signed = RoseGeometry.relative(relativeDeg)
    val whole = abs(signed).roundToInt()
    return when {
        whole == 0 -> "ahead"
        signed > 0 -> "$whole° right"
        else -> "$whole° left"
    }
}

private val CARDINALS = listOf("N", "E", "S", "W")
private const val RING = 0.92f
private const val NEEDLE = 0.92f
private const val GUIDANCE = 0.7f
private const val LETTERS = 0.78f
private const val CAR_AT = 0.55f
private const val LONG_TICK = 0.86f
private const val SHORT_TICK = 0.93f
private const val WEDGE_ALPHA = 0.2f
private const val QUARTER = 90.0
private const val TICKS = 12
private const val TICK_DEG = 30.0
private const val CARDINAL_EVERY = 3
