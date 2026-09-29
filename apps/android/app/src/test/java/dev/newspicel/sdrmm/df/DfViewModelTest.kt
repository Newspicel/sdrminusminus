package dev.newspicel.sdrmm.df

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.IntentFilter
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.TargetMode
import dev.newspicel.sdrmm.map.CameraFollow
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.Shadows.shadowOf

@RunWith(AndroidJUnit4::class)
class DfViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway()
    private val graph: AppGraph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core) }
    private val model by lazy {
        core.missions.value = Samples.missions(Samples.mission("d1", MissionKind.DF_DRIVE, "Kraken").copy(controls = listOf(MissionControl.TARGET_MODE, MissionControl.CLEAR_FUSION)))
        DfViewModel("d1", core, graph.settings, graph.sensors, graph.runner, graph.nav, graph.alerts, graph.router)
    }

    @Test
    fun navigate_uses_target() {
        val component = ComponentName("org.example.maps", "org.example.maps.Main")
        val packages = shadowOf(ApplicationProvider.getApplicationContext<android.app.Application>().packageManager)
        packages.addActivityIfNotPresent(component)
        packages.addIntentFilterForActivity(
            component,
            IntentFilter(Intent.ACTION_VIEW).apply {
                addCategory(Intent.CATEGORY_DEFAULT)
                addDataScheme("geo")
            },
        )
        core.df.value = Samples.df()
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        model.navigate(activity)
        val started = shadowOf(activity).nextStartedActivity
        val inner = started.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
        assertThat(inner?.dataString).isEqualTo("geo:52.520000,13.405000")
        assertThat(graph.nav.handedOff.value).containsExactly("d1")
    }

    @Test
    fun clear_requires_confirm() {
        model.askClear()
        assertThat(model.state.value.confirmClear).isTrue()
        assertThat(core.commands).isEmpty()
        model.confirmClear()
        assertThat(model.state.value.confirmClear).isFalse()
        assertThat(core.commands).containsExactly(MissionCommand.ClearFusion)
    }

    @Test
    fun mode() {
        model.setMode(TargetMode.DIRECT)
        assertThat(core.commands).containsExactly(MissionCommand.SetTargetMode(TargetMode.DIRECT))
    }

    @Test
    fun layers_follow_and_fit() {
        model.toggleLayer(MapLayer.Heat)
        assertThat(model.state.value.layers.heat).isFalse()
        assertThat(model.state.value.follow).isEqualTo(CameraFollow.UserHeading)
        model.cycleFollow()
        assertThat(model.state.value.follow).isEqualTo(CameraFollow.User)
        model.fit()
        assertThat(model.state.value.follow).isEqualTo(CameraFollow.Free)
        assertThat(model.state.value.fitRequest).isEqualTo(1)
        model.fitEmpty()
        assertThat(model.state.value.follow).isEqualTo(CameraFollow.User)
    }

    @Test
    fun other_missions_are_ignored() {
        core.df.value = Samples.df(mission = "other")
        assertThat(model.state.value.view).isNull()
        assertThat(model.state.value.title).isEqualTo("Kraken")
    }
}
