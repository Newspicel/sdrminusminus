package dev.newspicel.sdrmm.car

import android.content.res.Resources
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.df.GuidanceText
import dev.newspicel.sdrmm.df.RoseGeometry
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.ui.Format
import kotlin.math.roundToInt

data class CarPanel(
    val title: String,
    val text: String,
    val canNavigate: Boolean,
    val canCalibrate: Boolean,
    val canClear: Boolean,
    val roseBucket: Int,
) {
    companion object {
        private const val BUCKET_DEG = 5.0
        private const val BUCKETS = 1_000

        fun make(
            view: DfView?,
            pose: PoseView?,
            fix: LocationSample?,
            controls: List<MissionControl>,
            units: Units,
            core: CoreGateway,
            resources: Resources,
        ): CarPanel {
            val rose = RoseGeometry.model(view, pose)
            val title =
                if (view?.state == DfState.LIVE) {
                    "${Format.angle(rose.bearingDeg)}  ${Format.percent(view.confidence)}"
                } else {
                    view?.state?.let(GuidanceText::state)?.let(resources::getString) ?: resources.getString(R.string.df_waiting)
                }
            val target = view?.target
            val distance = if (fix != null && target != null) core.distanceM(LatLon(fix.lat, fix.lon), target.at) else null
            return CarPanel(
                title = title,
                text = GuidanceText.line(view, units, resources, distance),
                canNavigate = target != null,
                canCalibrate = MissionControl.CALIBRATE in controls,
                canClear = MissionControl.CLEAR_FUSION in controls,
                roseBucket = (bucket(rose.bearingDeg) * BUCKETS + bucket(rose.guidanceDeg)) * 2 + if (rose.headingUp) 1 else 0,
            )
        }

        private fun bucket(deg: Double?): Int = deg?.let { (it / BUCKET_DEG).roundToInt() % (360 / BUCKET_DEG.toInt()) + 1 } ?: 0
    }
}
