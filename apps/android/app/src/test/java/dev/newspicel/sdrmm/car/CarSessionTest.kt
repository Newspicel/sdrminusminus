package dev.newspicel.sdrmm.car

import android.content.Intent
import androidx.car.app.AppManager
import androidx.car.app.model.MessageTemplate
import androidx.car.app.navigation.model.MapWithContentTemplate
import androidx.car.app.testing.ScreenController
import androidx.car.app.testing.SessionController
import androidx.car.app.testing.TestAppManager
import androidx.car.app.testing.TestCarContext
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.Startup
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class CarSessionTest {
    @get:Rule val main = MainDispatcherRule()

    private val carContext = TestCarContext.createCarContext(ApplicationProvider.getApplicationContext())
    private val core = FakeCoreGateway()

    private fun firstTemplate(startup: Startup?): Any {
        val session = CarSession(startup)
        SessionController(session, carContext, Intent()).moveToState(Lifecycle.State.CREATED)
        val screen = session.onCreateScreen(Intent())
        ScreenController(screen).moveToState(Lifecycle.State.RESUMED)
        return screen.onGetTemplate()
    }

    @Test
    fun unpaired() {
        val template = firstTemplate(Startup.Ready(TestAppGraph.create(ApplicationProvider.getApplicationContext(), core)))
        assertThat(template).isInstanceOf(MessageTemplate::class.java)
        assertThat((template as MessageTemplate).message.toString()).isEqualTo("Pair on phone")
    }

    @Test
    fun core_failed() {
        val template = firstTemplate(Startup.Failed("no library")) as MessageTemplate
        assertThat(template.message.toString()).isEqualTo("Core failed")
    }

    @Test
    fun paired_shows_missions_and_registers_the_map() {
        core.servers += Samples.server()
        val template = firstTemplate(Startup.Ready(TestAppGraph.create(ApplicationProvider.getApplicationContext(), core)))
        assertThat(template).isInstanceOf(MapWithContentTemplate::class.java)
        val manager = carContext.getCarService(AppManager::class.java) as TestAppManager
        assertThat(manager.surfaceCallback).isInstanceOf(CarMapRenderer::class.java)
    }
}
