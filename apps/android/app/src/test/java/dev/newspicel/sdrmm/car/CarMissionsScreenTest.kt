package dev.newspicel.sdrmm.car

import androidx.car.app.OnDoneCallback
import androidx.car.app.model.ListTemplate
import androidx.car.app.model.Row
import androidx.car.app.navigation.model.MapWithContentTemplate
import androidx.car.app.serialization.Bundleable
import androidx.car.app.testing.ScreenController
import androidx.car.app.testing.TestCarContext
import androidx.car.app.testing.TestScreenManager
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class CarMissionsScreenTest {
    @get:Rule val main = MainDispatcherRule()

    private val carContext = TestCarContext.createCarContext(ApplicationProvider.getApplicationContext())
    private val core = FakeCoreGateway().apply { link.value = LinkState.Online("Bench") }
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core) }

    private fun rows(): List<Row> {
        val screen = CarMissionsScreen(carContext, graph, CarMapRenderer(carContext, graph, CoroutineScope(Dispatchers.Main), dark = false))
        ScreenController(screen).moveToState(Lifecycle.State.RESUMED)
        val template = screen.onGetTemplate() as MapWithContentTemplate
        val list = template.contentTemplate as ListTemplate
        return list.singleList?.items.orEmpty().map { it as Row }
    }

    @Test
    fun df_only() {
        core.missions.value = Samples.missions(Samples.mission("h", MissionKind.HUNT, "Beacon"), Samples.mission("d", MissionKind.DF_DRIVE, "Kraken"))
        val rows = rows()
        assertThat(rows.map { it.title.toString() }).containsExactly("Kraken")
    }

    @Test
    fun no_df_missions_and_offline() {
        core.missions.value = Samples.missions(Samples.mission("h", MissionKind.HUNT, "Beacon"))
        assertThat(rows().single().title.toString()).isEqualTo("No DF missions")
        core.link.value = LinkState.Offline
        assertThat(rows().single().title.toString()).isEqualTo("Offline")
    }

    @Test
    fun a_row_opens_the_df_screen() {
        core.missions.value = Samples.missions(Samples.mission("d", MissionKind.DF_DRIVE, "Kraken"))
        val row = rows().single()
        row.onClickDelegate?.sendClick(
            object : OnDoneCallback {
                override fun onSuccess(response: Bundleable?) = Unit

                override fun onFailure(response: Bundleable) = Unit
            },
        )
        assertThat(core.calls).contains("openMission:d")
        val screens = carContext.getCarService(TestScreenManager::class.java).screensPushed
        assertThat(screens.last()).isInstanceOf(CarDfScreen::class.java)
    }
}
