package dev.newspicel.sdrmm.speech

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Test

class SpeechTextTest {
    @Test
    fun retarget() {
        val notice = Samples.notice(RetargetReason.MOVED)
        assertThat(SpeechText.retarget(notice, 1234.0, 45.0, Units.Metric)).isEqualTo("New target. 1.2 kilometers north east.")
        assertThat(SpeechText.retarget(notice, null, null, Units.Metric)).isEqualTo("New target.")
    }

    @Test
    fun mode_changed() {
        val closing = Samples.notice(RetargetReason.KIND_CHANGED, kind = GuidanceKind.ESTIMATE)
        assertThat(SpeechText.retarget(closing, 340.0, 10.0, Units.Metric)).isEqualTo("Closing in. 340 meters.")
        val cross = Samples.notice(RetargetReason.KIND_CHANGED, kind = GuidanceKind.PROBE)
        assertThat(SpeechText.retarget(cross, 340.0, 10.0, Units.Metric)).isEqualTo("Cross the bearing.")
    }

    @Test
    fun spoken_distance_uses_long_names() {
        assertThat(SpeechText.spokenDistance(1000.0, Units.Metric)).isEqualTo("1 kilometer")
        assertThat(SpeechText.spokenDistance(43.0, Units.Metric)).isEqualTo("45 meters")
        assertThat(SpeechText.spokenDistance(150.0, Units.Imperial)).isEqualTo("500 feet")
        assertThat(SpeechText.spokenDistance(1609.344, Units.Imperial)).isEqualTo("1 mile")
        assertThat(SpeechText.spokenDistance(2000.0, Units.Imperial)).isEqualTo("1.2 miles")
    }

    @Test
    fun compass_sectors_center_on_each_direction() {
        assertThat(SpeechText.compass8(0.0)).isEqualTo("north")
        assertThat(SpeechText.compass8(22.0)).isEqualTo("north")
        assertThat(SpeechText.compass8(23.0)).isEqualTo("north east")
        assertThat(SpeechText.compass8(-90.0)).isEqualTo("west")
        assertThat(SpeechText.compass8(350.0)).isEqualTo("north")
    }
}
