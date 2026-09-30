package dev.newspicel.sdrmm.radar

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class RadarViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway().apply { missions.value = Samples.missions(Samples.mission("r1", MissionKind.RADAR_WATCH, "FM")) }
    private val router = NoticeRouter(core, FakeSettingsStore(), Samples.retargets())
    private val model by lazy { RadarViewModel("r1", core, router, main.dispatcher) }

    @Test
    fun rows() {
        val tracks = (1..10).map { Samples.track(it, snr = it.toFloat(), doppler = if (it == 7) 35f else -35f, closing = it != 7) }
        core.radar.value = RadarView("r1", 12u, tracks, stale = true, problems = emptyList())
        val rows = model.state.value.rows
        assertThat(rows).hasSize(8)
        assertThat(rows.map { it.name }).containsExactly("T10", "T09", "T08", "T07", "T06", "T05", "T04", "T03").inOrder()
        val seven = rows.first { it.name == "T07" }
        assertThat(seven.doppler).isEqualTo("+35 Hz")
        assertThat(seven.range).isEqualTo("12.4 km")
        assertThat(seven.motion).isEqualTo(R.string.radar_opening)
        assertThat(rows.first().motion).isEqualTo(R.string.radar_closing)
        assertThat(model.state.value.stale).isTrue()
        assertThat(model.state.value.echoes).isEqualTo(12u)
    }

    @Test
    fun images_decode_and_bad_ones_are_visible() {
        core.radarImage.value = RgbaImage(2u, 2u, ByteArray(16) { 0x7f }, 30f, 200f)
        assertThat(model.state.value.frame).isNotNull()
        assertThat(model.state.value.frameVersion).isEqualTo(1)
        assertThat(model.state.value.rangeMaxKm).isEqualTo(30f)
        core.radarImage.value = RgbaImage(2u, 2u, ByteArray(3), 30f, 200f)
        assertThat(model.state.value.imageFailed).isTrue()
        assertThat(router.banners.value).hasSize(1)
    }

    @Test
    fun other_missions_are_ignored() {
        core.radar.value = RadarView("other", 3u, listOf(Samples.track(1, 3f)), false, emptyList())
        assertThat(model.state.value.rows).isEmpty()
    }
}
