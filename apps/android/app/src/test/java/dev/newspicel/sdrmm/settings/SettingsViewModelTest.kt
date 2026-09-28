package dev.newspicel.sdrmm.settings

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.ffi.AlignState
import dev.newspicel.sdrmm.ffi.HeadingMode
import dev.newspicel.sdrmm.ffi.Mount
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.Destination
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(AndroidJUnit4::class)
class SettingsViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway().apply { servers += Samples.server() }
    private val settings = FakeSettingsStore(AppSettings(phoneName = "Pixel", activeServerId = "s1"))
    private val graph: AppGraph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core, settings) }
    private val model by lazy { SettingsViewModel(core, settings, graph.sensors, graph.navigator, graph.router, graph::activate, "1.2.3") }

    @Test
    fun pose_settings() = runTest {
        syncPoseSettings(backgroundScope, settings, core)
        model.setMount(Mount.FLAT)
        model.setSource(HeadingMode.COMPASS)
        model.stepOffset(5)
        model.stepOffset(-200)
        runCurrent()
        val last = core.poseSettings.last()
        assertThat(last.mount).isEqualTo(Mount.FLAT)
        assertThat(last.headingMode).isEqualTo(HeadingMode.COMPASS)
        assertThat(last.mountOffsetDeg).isEqualTo(-180.0)
        assertThat(last.sharePose).isTrue()
    }

    @Test
    fun alignment_result_is_saved() = runTest {
        syncPoseSettings(backgroundScope, settings, core)
        model.align(true)
        core.pose.value = Samples.pose().copy(align = AlignState.Done(offsetDeg = 7.0))
        runCurrent()
        assertThat(settings.settings.value.mountOffsetDeg).isEqualTo(7.0)
        assertThat(core.calls).contains("startAlign")
    }

    @Test
    fun forget() {
        assertThat(graph.navigator.backStack).containsExactly(Destination.Missions)
        model.askForget(Samples.server())
        assertThat(model.state.value.confirmForget).isEqualTo(Samples.server())
        model.confirmForget()
        assertThat(core.calls).containsAtLeast("forgetServer:s1", "disconnect").inOrder()
        assertThat(settings.settings.value.activeServerId).isNull()
        assertThat(model.state.value.servers).isEmpty()
        assertThat(model.state.value.confirmForget).isNull()
        assertThat(graph.navigator.backStack).containsExactly(Destination.Pair)
    }

    @Test
    fun settings_rows_write_through() {
        model.setName("x".repeat(80))
        model.setUnits(Units.Metric)
        model.setVoice(false)
        model.setNav(NavChoice.GoogleMaps)
        model.setKeepOn(false)
        model.setTiles(false)
        val saved = settings.settings.value
        assertThat(saved.phoneName).hasLength(64)
        assertThat(saved.units).isEqualTo(Units.Metric)
        assertThat(saved.voice).isFalse()
        assertThat(saved.navChoice).isEqualTo(NavChoice.GoogleMaps)
        assertThat(saved.keepScreenOn).isFalse()
        assertThat(saved.mapTiles).isFalse()
        assertThat(model.state.value.appVersion).isEqualTo("1.2.3")
        assertThat(model.state.value.about).isEqualTo(core.about)
    }

    @Test
    fun add_server_and_licenses_navigate() {
        model.addServer()
        assertThat(graph.navigator.top()).isEqualTo(Destination.Pair)
        graph.navigator.back()
        model.openLicenses()
        assertThat(graph.navigator.top()).isEqualTo(Destination.Licenses)
    }
}
