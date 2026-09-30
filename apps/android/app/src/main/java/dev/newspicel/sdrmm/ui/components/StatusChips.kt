package dev.newspicel.sdrmm.ui.components

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.HeadingSourceKind
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.mission.BackgroundState
import dev.newspicel.sdrmm.sensors.LocationAccess
import dev.newspicel.sdrmm.sensors.SensorStatus
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import kotlin.math.roundToInt

enum class ChipAction { AllowLocation, LocationSettings, AllowNotifications }

data class Chip(
    val text: UiText,
    val problem: Boolean,
    val action: ChipAction? = null,
)

object Chips {
    fun of(
        link: LinkState,
        status: SensorStatus,
        pose: PoseView?,
    ): List<Chip> = listOfNotNull(link(link), sharing(pose), location(status), compass(status), heading(pose))

    fun link(link: LinkState): Chip = when (link) {
        is LinkState.Online -> Chip(UiText.Res(R.string.link_online), problem = false)
        is LinkState.Connecting -> Chip(UiText.Res(R.string.link_connecting), problem = true)
        is LinkState.Refused -> Chip(UiText.Raw(link.text), problem = true)
        LinkState.Offline -> Chip(UiText.Res(R.string.link_offline), problem = true)
    }

    fun mission(
        link: LinkState,
        status: SensorStatus,
        pose: PoseView?,
        background: BackgroundState,
    ): List<Chip> = of(link, status, pose) + listOfNotNull(background(background))

    fun background(background: BackgroundState): Chip? = if (background is BackgroundState.Off) Chip(UiText.Res(R.string.chip_background_off), problem = true) else null

    fun alerts(off: Boolean): Chip? = if (off) Chip(UiText.Res(R.string.chip_alerts_off), true, ChipAction.AllowNotifications) else null

    fun tiles(failed: Boolean): Chip? = if (failed) Chip(UiText.Res(R.string.chip_no_tiles), problem = true) else null

    fun sharing(pose: PoseView?): Chip? = if (pose?.sending == true) Chip(UiText.Res(R.string.chip_sharing), problem = false) else null

    fun location(status: SensorStatus): Chip? = when {
        status.access == LocationAccess.Off -> Chip(UiText.Res(R.string.chip_location_off), true, ChipAction.AllowLocation)
        !status.gpsEnabled -> Chip(UiText.Res(R.string.chip_location_off), true, ChipAction.LocationSettings)
        !status.precise -> Chip(UiText.Res(R.string.chip_approx), true, ChipAction.AllowLocation)
        !status.fix -> Chip(UiText.Res(R.string.chip_no_fix), true)
        else -> null
    }

    private fun compass(status: SensorStatus): Chip? = when {
        !status.compass -> Chip(UiText.Res(R.string.chip_no_compass), true)
        !status.compassReliable -> Chip(UiText.Res(R.string.chip_calibrate_compass), true)
        else -> null
    }

    fun heading(pose: PoseView?): Chip {
        val deg = pose?.headingDeg
        if (pose == null || deg == null || pose.source == HeadingSourceKind.NONE) return Chip(UiText.Res(R.string.chip_no_heading), true)
        val source =
            when (pose.source) {
                HeadingSourceKind.COMPASS -> R.string.source_compass
                HeadingSourceKind.COURSE -> R.string.source_course
                else -> R.string.source_fused
            }
        val accuracy = pose.accuracyDeg?.takeIf { it.isFinite() } ?: return Chip(UiText.Res(source), problem = false)
        return Chip(UiText.Res(R.string.chip_heading, listOf(UiText.Res(source), accuracy.roundToInt().toString())), problem = false)
    }
}

@Composable
fun StatusChips(
    chips: List<Chip>,
    onAction: (ChipAction) -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = modifier.horizontalScroll(rememberScrollState()),
    ) {
        val warn = LocalStatusColors.current.warn
        for (chip in chips) {
            AssistChip(
                onClick = { chip.action?.let(onAction) },
                enabled = true,
                label = { Text(chip.text.text()) },
                colors =
                if (chip.problem) {
                    AssistChipDefaults.assistChipColors(labelColor = warn)
                } else {
                    AssistChipDefaults.assistChipColors()
                },
            )
        }
    }
}
