package dev.newspicel.sdrmm.df

import androidx.compose.ui.geometry.Offset
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.PoseView
import kotlin.math.cos
import kotlin.math.sin

data class RoseModel(
    val bearingDeg: Double?,
    val sigmaDeg: Double?,
    val guidanceDeg: Double?,
    val northDeg: Double,
    val headingUp: Boolean,
)

object RoseGeometry {
    private const val FULL = 360.0

    fun model(
        view: DfView?,
        pose: PoseView?,
    ): RoseModel {
        val heading = pose?.headingDeg
        val relative = view?.bearingRelDeg
        val guidance = view?.guidance
        val live = view?.state == DfState.LIVE
        if (heading == null || relative == null) {
            return RoseModel(
                bearingDeg = view?.bearingTrueDeg?.toDouble()?.takeIf { live }?.let(::wrap),
                sigmaDeg = view?.sigmaDeg?.toDouble(),
                guidanceDeg = guidance?.headingTrueDeg?.let(::wrap),
                northDeg = 0.0,
                headingUp = false,
            )
        }
        return RoseModel(
            bearingDeg = relative.toDouble().takeIf { live }?.let(::wrap),
            sigmaDeg = view.sigmaDeg?.toDouble(),
            guidanceDeg = guidance?.headingRelDeg?.let(::wrap),
            northDeg = wrap(FULL - heading),
            headingUp = true,
        )
    }

    fun point(
        angleDeg: Double,
        radius: Float,
        cx: Float,
        cy: Float,
    ): Offset {
        val theta = Math.toRadians(angleDeg)
        return Offset(cx + radius * sin(theta).toFloat(), cy - radius * cos(theta).toFloat())
    }

    fun wrap(deg: Double): Double = ((deg % FULL) + FULL) % FULL

    fun relative(bearingDeg: Double): Double {
        val wrapped = wrap(bearingDeg)
        return if (wrapped > FULL / 2) wrapped - FULL else wrapped
    }
}
