package dev.newspicel.sdrmm.df

import android.content.res.Resources
import androidx.annotation.StringRes
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.ui.Format

object GuidanceText {
    fun line(
        view: DfView?,
        units: Units,
        resources: Resources,
        distanceM: Double? = null,
    ): String {
        val guidance = view?.guidance ?: return resources.getString(R.string.no_guidance)
        val distance = Format.distance(distanceM ?: guidance.distanceM, units)
        return when (guidance.kind) {
            GuidanceKind.PROBE -> resources.getString(R.string.guidance_cross, Format.angle(guidance.headingTrueDeg), distance)
            GuidanceKind.ESTIMATE -> resources.getString(R.string.guidance_approach, distance)
        }
    }

    @StringRes
    fun state(state: DfState): Int? = when (state) {
        DfState.WAITING -> R.string.df_waiting
        DfState.CALIBRATING -> R.string.df_calibrating
        DfState.PHASE_UNKNOWN -> R.string.df_phase_unknown
        DfState.NO_HEADING -> R.string.df_no_heading
        DfState.SQUELCHED -> R.string.df_squelched
        DfState.LIVE -> null
    }
}
