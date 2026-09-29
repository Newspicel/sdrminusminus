package dev.newspicel.sdrmm.mission

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.nav.HandoffResult
import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.ui.components.UiText
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class NoticeRouterTest {
    private val core = FakeCoreGateway()
    private val settings = FakeSettingsStore(AppSettings(phoneName = "Pixel", units = Units.Metric))
    private val handedOff = MutableStateFlow<Set<String>>(emptySet())
    private val resumed = MutableStateFlow(true)
    private val spoken = mutableListOf<String>()
    private val posted = mutableListOf<Pair<RetargetNotice, String>>()
    private val router =
        NoticeRouter(
            core,
            settings,
            Samples.retargets(
                lastFix = MutableStateFlow(Samples.fix()),
                handedOff = handedOff,
                appResumed = resumed,
                speak = { spoken += it },
                notify = { notice, text -> posted += notice to text },
            ),
        )

    @Test
    fun retarget_rules() = runTest {
        router.run(backgroundScope)
        core.retargets.emit(Samples.notice(RetargetReason.FIRST))
        runCurrent()
        assertThat(router.banners.value.single()).isInstanceOf(Banner.Retarget::class.java)
        assertThat(spoken).isEmpty()
        assertThat(posted).isEmpty()

        resumed.value = false
        core.retargets.emit(Samples.notice(RetargetReason.MOVED))
        runCurrent()
        assertThat(spoken).hasSize(1)
        assertThat(spoken.single()).startsWith("New target.")
        assertThat(posted).isEmpty()

        handedOff.value = setOf("d1")
        core.retargets.emit(Samples.notice(RetargetReason.MOVED))
        runCurrent()
        assertThat(posted).hasSize(1)
        assertThat(posted.single().second).isEqualTo((router.banners.value.last() as Banner.Retarget).distanceText)
        assertThat(spoken).hasSize(2)

        resumed.value = true
        core.retargets.emit(Samples.notice(RetargetReason.MOVED))
        runCurrent()
        assertThat(posted).hasSize(1)
        assertThat(router.banners.value.filterIsInstance<Banner.Retarget>()).hasSize(4)
    }

    @Test
    fun handoff_results_are_visible() {
        router.handoff(HandoffResult.Opened)
        assertThat(router.banners.value).isEmpty()
        router.handoff(HandoffResult.FellBack)
        router.handoff(HandoffResult.Failed(R.string.no_map_app))
        val banners = router.banners.value.map { it as Banner.Text }
        assertThat(banners.map { it.level }).containsExactly(NoticeLevel.INFO, NoticeLevel.ERROR).inOrder()
        assertThat(banners.map { it.text }).containsExactly(UiText.Res(R.string.google_maps_missing), UiText.Res(R.string.no_map_app)).inOrder()
    }

    @Test
    fun voice_off() = runTest {
        settings.settings.value = settings.settings.value.copy(voice = false)
        router.run(backgroundScope)
        core.retargets.emit(Samples.notice(RetargetReason.MOVED))
        runCurrent()
        assertThat(spoken).isEmpty()
        assertThat(router.banners.value).hasSize(1)
    }

    @Test
    fun banner_distance_comes_from_the_last_fix() = runTest {
        router.run(backgroundScope)
        core.retargets.emit(Samples.notice(RetargetReason.MOVED))
        runCurrent()
        val banner = router.banners.value.single() as Banner.Retarget
        assertThat(banner.distanceText).isEqualTo("1.5 km")
        assertThat(banner.level).isEqualTo(NoticeLevel.INFO)
    }
}
