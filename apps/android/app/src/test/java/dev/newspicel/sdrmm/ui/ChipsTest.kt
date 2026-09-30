package dev.newspicel.sdrmm.ui

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.HeadingSourceKind
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.RefusalKind
import dev.newspicel.sdrmm.sensors.LocationAccess
import dev.newspicel.sdrmm.sensors.SensorStatus
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.ui.components.ChipAction
import dev.newspicel.sdrmm.ui.components.Chips
import dev.newspicel.sdrmm.ui.components.UiText
import org.junit.Test

class ChipsTest {
    private val healthy = SensorStatus.Unknown.copy(access = LocationAccess.WhileUsing, precise = true, fix = true)

    @Test
    fun location_problems_in_order() {
        assertThat(Chips.location(SensorStatus.Unknown)?.action).isEqualTo(ChipAction.AllowLocation)
        assertThat(Chips.location(healthy.copy(gpsEnabled = false))?.action).isEqualTo(ChipAction.LocationSettings)
        assertThat(Chips.location(healthy.copy(precise = false))?.text).isEqualTo(UiText.Res(R.string.chip_approx))
        assertThat(Chips.location(healthy.copy(fix = false))?.text).isEqualTo(UiText.Res(R.string.chip_no_fix))
        assertThat(Chips.location(healthy)).isNull()
    }

    @Test
    fun heading_names_its_source() {
        assertThat(Chips.heading(null).text).isEqualTo(UiText.Res(R.string.chip_no_heading))
        val chip = Chips.heading(Samples.pose(accuracy = 11.6, source = HeadingSourceKind.COURSE))
        assertThat(chip.text).isEqualTo(UiText.Res(R.string.chip_heading, listOf(UiText.Res(R.string.source_course), "12")))
        assertThat(chip.problem).isFalse()
        val course = Chips.heading(Samples.pose(accuracy = null, source = HeadingSourceKind.COURSE))
        assertThat(course.text).isEqualTo(UiText.Res(R.string.source_course))
        assertThat(Chips.heading(Samples.pose(accuracy = Double.NaN)).text).isEqualTo(UiText.Res(R.string.source_compass))
        assertThat(Chips.heading(Samples.pose(source = HeadingSourceKind.NONE)).text).isEqualTo(UiText.Res(R.string.chip_no_heading))
    }

    @Test
    fun sharing_and_compass_chips() {
        val chips = Chips.of(LinkState.Online("Bench"), healthy.copy(compass = false), Samples.pose(sending = true))
        assertThat(chips.map { it.text })
            .containsExactly(
                UiText.Res(R.string.link_online),
                UiText.Res(R.string.chip_sharing),
                UiText.Res(R.string.chip_no_compass),
                UiText.Res(R.string.chip_heading, listOf(UiText.Res(R.string.source_compass), "12")),
            ).inOrder()
    }

    @Test
    fun a_refused_link_shows_the_reason() {
        val chip = Chips.link(LinkState.Refused(RefusalKind.KEY_MISMATCH, "Key changed"))
        assertThat(chip.text).isEqualTo(UiText.Raw("Key changed"))
        assertThat(chip.problem).isTrue()
        assertThat(Chips.link(LinkState.Connecting(3u, "10.0.2.2:8443")).text).isEqualTo(UiText.Res(R.string.link_connecting))
        assertThat(Chips.link(LinkState.Offline).problem).isTrue()
    }
}
