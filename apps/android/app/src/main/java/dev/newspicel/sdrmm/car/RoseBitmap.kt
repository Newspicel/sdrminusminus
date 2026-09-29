package dev.newspicel.sdrmm.car

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import androidx.compose.ui.graphics.toArgb
import androidx.core.graphics.createBitmap
import dev.newspicel.sdrmm.df.RoseGeometry
import dev.newspicel.sdrmm.df.RoseModel
import dev.newspicel.sdrmm.ui.theme.DarkColors
import dev.newspicel.sdrmm.ui.theme.DarkStatus
import dev.newspicel.sdrmm.ui.theme.LightColors
import dev.newspicel.sdrmm.ui.theme.LightStatus

object RoseBitmap {
    fun draw(
        model: RoseModel,
        sizePx: Int,
        dark: Boolean,
    ): Bitmap {
        val colors = if (dark) DarkColors else LightColors
        val status = if (dark) DarkStatus else LightStatus
        val bitmap = createBitmap(sizePx, sizePx)
        val canvas = Canvas(bitmap)
        val c = sizePx / 2f
        val r = c * RING
        val stroke = sizePx / STROKE_DIVISOR
        val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = stroke
        paint.color = colors.outline.toArgb()
        canvas.drawCircle(c, c, r, paint)
        paint.color = colors.onSurfaceVariant.toArgb()
        for (step in 0 until TICKS) {
            val angle = model.northDeg + step * TICK_DEG
            val inner = RoseGeometry.point(angle, r * if (step % CARDINAL_EVERY == 0) LONG_TICK else SHORT_TICK, c, c)
            val outer = RoseGeometry.point(angle, r, c, c)
            canvas.drawLine(inner.x, inner.y, outer.x, outer.y, paint)
        }
        text(canvas, model, r, c, colors.onBackground.toArgb())
        val bearing = model.bearingDeg
        if (bearing != null) {
            model.sigmaDeg?.takeIf { it.isFinite() && it > 0 }?.let { sigma ->
                val wedge = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colors.primary.copy(alpha = WEDGE_ALPHA).toArgb() }
                canvas.drawArc(RectF(c - r, c - r, c + r, c + r), (bearing - sigma - QUARTER).toFloat(), (2 * sigma).toFloat(), true, wedge)
            }
            line(canvas, bearing, r * NEEDLE, c, colors.primary.toArgb(), stroke * NEEDLE_WIDTH)
        }
        model.guidanceDeg?.let { line(canvas, it, r * GUIDANCE, c, status.warn.toArgb(), stroke * GUIDANCE_WIDTH) }
        paint.style = Paint.Style.FILL
        paint.color = colors.onBackground.toArgb()
        canvas.drawCircle(c, c, stroke * DOT, paint)
        return bitmap
    }

    private fun line(
        canvas: Canvas,
        angleDeg: Double,
        length: Float,
        c: Float,
        color: Int,
        width: Float,
    ) {
        val paint =
            Paint(Paint.ANTI_ALIAS_FLAG).apply {
                this.color = color
                strokeWidth = width
                strokeCap = Paint.Cap.ROUND
            }
        val end = RoseGeometry.point(angleDeg, length, c, c)
        canvas.drawLine(c, c, end.x, end.y, paint)
    }

    private fun text(
        canvas: Canvas,
        model: RoseModel,
        r: Float,
        c: Float,
        color: Int,
    ) {
        val paint =
            Paint(Paint.ANTI_ALIAS_FLAG).apply {
                this.color = color
                textSize = r * LETTER_SIZE
                textAlign = Paint.Align.CENTER
            }
        CARDINALS.forEachIndexed { index, letter ->
            val at = RoseGeometry.point(model.northDeg + index * QUARTER, r * LETTERS, c, c)
            canvas.drawText(letter, at.x, at.y + paint.textSize / 3, paint)
        }
    }

    private val CARDINALS = listOf("N", "E", "S", "W")
    private const val RING = 0.92f
    private const val STROKE_DIVISOR = 128f
    private const val NEEDLE = 0.92f
    private const val GUIDANCE = 0.7f
    private const val NEEDLE_WIDTH = 3f
    private const val GUIDANCE_WIDTH = 5f
    private const val DOT = 3f
    private const val LETTERS = 0.74f
    private const val LETTER_SIZE = 0.18f
    private const val LONG_TICK = 0.84f
    private const val SHORT_TICK = 0.92f
    private const val WEDGE_ALPHA = 0.2f
    private const val QUARTER = 90.0
    private const val TICKS = 12
    private const val TICK_DEG = 30.0
    private const val CARDINAL_EVERY = 3
}
