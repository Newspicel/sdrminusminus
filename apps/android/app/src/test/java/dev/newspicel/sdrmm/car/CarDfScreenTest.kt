package dev.newspicel.sdrmm.car

import androidx.car.app.AppManager
import androidx.car.app.CarContext
import androidx.car.app.OnDoneCallback
import androidx.car.app.model.Action
import androidx.car.app.navigation.model.MessageInfo
import androidx.car.app.navigation.model.NavigationTemplate
import androidx.car.app.serialization.Bundleable
import androidx.car.app.testing.ScreenController
import androidx.car.app.testing.TestAppManager
import androidx.car.app.testing.TestCarContext
import androidx.car.app.testing.TestScreenManager
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.settings.Units
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.shadows.ShadowLooper
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class CarDfScreenTest {
    private val carContext = TestCarContext.createCarContext(ApplicationProvider.getApplicationContext())
    private val core =
        FakeCoreGateway().apply {
            missions.value = Samples.missions(Samples.mission("d1", MissionKind.DF_DRIVE, "Kraken", DF_CONTROLS))
            pose.value = Samples.pose(heading = null)
        }
    private val graph by lazy {
        TestAppGraph.create(ApplicationProvider.getApplicationContext(), core, FakeSettingsStore(AppSettings(phoneName = "Pixel", units = Units.Metric)))
    }

    private fun controller(): ScreenController {
        graph.runner.open("d1")
        val screen = CarDfScreen(carContext, graph, CarMapRenderer(carContext, graph, CoroutineScope(Dispatchers.Main), dark = false), "d1")
        return ScreenController(screen).moveToState(Lifecycle.State.RESUMED)
    }

    private fun Action.click() {
        onClickDelegate?.sendClick(
            object : OnDoneCallback {
                override fun onSuccess(response: Bundleable?) = Unit

                override fun onFailure(response: Bundleable) = Unit
            },
        )
        ShadowLooper.idleMainLooper()
    }

    private fun template(controller: ScreenController): NavigationTemplate = controller.screen.onGetTemplate() as NavigationTemplate

    private fun action(
        controller: ScreenController,
        title: String,
    ): Action = template(controller).actionStrip?.actions.orEmpty().first { it.title.toString() == title }

    @Test
    fun panel() {
        core.df.value = Samples.df()
        val info = template(controller()).navigationInfo as MessageInfo
        assertThat(info.title.toString()).isEqualTo("137°  62%")
        assertThat(info.text.toString()).isEqualTo("Cross 215° · 1.2 km")
        assertThat(info.image).isNotNull()
    }

    @Test
    fun navigate_intent() {
        core.df.value = Samples.df()
        val controller = controller()
        action(controller, "Navigate").click()
        val intent = carContext.startCarAppIntents.single()
        assertThat(intent.action).isEqualTo(CarContext.ACTION_NAVIGATE)
        assertThat(intent.dataString).isEqualTo("geo:52.520000,13.405000")
        assertThat(graph.nav.handedOff.value).containsExactly("d1")
    }

    @Test
    fun clear_no_dialog() {
        core.df.value = Samples.df()
        val controller = controller()
        action(controller, "Clear").click()
        assertThat(core.calls).contains("send:ClearFusion")
        val toasts = (carContext.getCarService(AppManager::class.java) as TestAppManager).toastsShown
        assertThat(toasts.map { it.toString() }).contains("Fusion cleared")
    }

    @Test
    fun only_the_missions_controls_are_offered() {
        core.df.value = Samples.df()
        val controller = controller()
        val titles = { template(controller).actionStrip?.actions.orEmpty().map { it.title.toString() } }
        assertThat(titles()).containsExactly("Navigate", "Calibrate", "Clear").inOrder()
        core.missions.value = Samples.missions(Samples.mission("d1", MissionKind.DF_DRIVE, "Fusion", listOf(MissionControl.TARGET_MODE)))
        ShadowLooper.idleMainLooper(2, TimeUnit.SECONDS)
        assertThat(titles()).containsExactly("Navigate")
    }

    @Test
    fun a_closed_mission_leaves_the_car_screen() {
        core.df.value = Samples.df()
        ScreenController(MessageScreen(carContext, R.string.car_no_df, null)).moveToState(Lifecycle.State.RESUMED)
        val controller = controller()
        ShadowLooper.idleMainLooper()
        val manager = carContext.getCarService(TestScreenManager::class.java)
        assertThat(manager.screensRemoved).doesNotContain(controller.screen)
        graph.runner.close()
        ShadowLooper.idleMainLooper()
        assertThat(manager.screensRemoved).contains(controller.screen)
    }

    @Test
    fun invalidate_throttled() {
        core.df.value = Samples.df()
        val controller = controller()
        controller.screen.invalidate()
        val before = controller.templatesReturned.size
        assertThat(before).isEqualTo(1)
        repeat(10) { index ->
            core.df.value = Samples.df(bearingTrue = 10f * index)
            ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS)
        }
        ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS)
        assertThat(controller.templatesReturned.size - before).isAtMost(2)
    }

    private companion object {
        val DF_CONTROLS = listOf(MissionControl.CALIBRATE, MissionControl.CLEAR_FUSION, MissionControl.TARGET_MODE)
    }
}
