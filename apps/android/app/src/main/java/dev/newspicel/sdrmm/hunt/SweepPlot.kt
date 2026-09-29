package dev.newspicel.sdrmm.hunt

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.newspicel.sdrmm.ffi.SweepView
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import kotlin.math.cos
import kotlin.math.sin

data class SweepBar(
    val centerDeg: Double,
    val spanDeg: Double,
    val fraction: Float,
)

object SweepGeometry {
    private const val FULL = 360.0
    private const val MAX_LEVEL = 255f

    fun screenDeg(
        trueDeg: Double,
        headingDeg: Double?,
    ): Double = ((trueDeg - (headingDeg ?: 0.0)) % FULL + FULL) % FULL

    fun bars(
        bins: ByteArray,
        headingDeg: Double?,
    ): List<SweepBar> {
        if (bins.isEmpty()) return emptyList()
        val span = FULL / bins.size
        return bins.mapIndexed { index, level ->
            SweepBar(screenDeg(index * span + span / 2, headingDeg), span, (level.toInt() and 0xFF) / MAX_LEVEL)
        }
    }

    fun marker(
        peakDeg: Float?,
        headingDeg: Double?,
    ): Double? = peakDeg?.takeIf { it.isFinite() }?.let { screenDeg(it.toDouble(), headingDeg) }

    fun point(
        angleDeg: Double,
        radius: Float,
        center: Offset,
    ): Offset {
        val theta = Math.toRadians(angleDeg)
        return Offset(center.x + radius * sin(theta).toFloat(), center.y - radius * cos(theta).toFloat())
    }
}

@Composable
fun SweepPlot(
    sweep: SweepView,
    headingDeg: Double?,
    description: String,
    modifier: Modifier = Modifier,
) {
    val ring = MaterialTheme.colorScheme.outline
    val accent = MaterialTheme.colorScheme.primary
    val warn = LocalStatusColors.current.warn
    val ink = MaterialTheme.colorScheme.onBackground
    val bars = SweepGeometry.bars(sweep.bins, headingDeg)
    val marker = SweepGeometry.marker(sweep.peakDeg, headingDeg)
    val north = SweepGeometry.screenDeg(0.0, headingDeg)
    Canvas(
        modifier.aspectRatio(1f).semantics {
            contentDescription = "$description ${Format.angle(sweep.peakDeg?.toDouble())}"
        },
    ) {
        val radius = size.minDimension / 2 * RING
        drawCircle(ring, radius, center, style = Stroke(1.5.dp.toPx()))
        for (bar in bars) {
            if (bar.fraction <= 0f) continue
            val r = radius * bar.fraction
            drawArc(
                color = accent.copy(alpha = BAR_ALPHA),
                startAngle = (bar.centerDeg - bar.spanDeg / 2 - QUARTER).toFloat(),
                sweepAngle = bar.spanDeg.toFloat(),
                useCenter = true,
                topLeft = Offset(center.x - r, center.y - r),
                size = Size(r * 2, r * 2),
            )
        }
        drawLine(ink, SweepGeometry.point(north, radius * NORTH_INNER, center), SweepGeometry.point(north, radius, center), 2.dp.toPx())
        marker?.let {
            drawLine(warn, center, SweepGeometry.point(it, radius * NEEDLE, center), 4.dp.toPx(), cap = StrokeCap.Round)
        }
        drawCircle(ink, 3.dp.toPx(), center)
    }
}

private const val RING = 0.95f
private const val BAR_ALPHA = 0.6f
private const val QUARTER = 90.0
private const val NEEDLE = 0.92f
private const val NORTH_INNER = 0.8f
