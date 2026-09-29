package dev.newspicel.sdrmm.mission

import android.Manifest
import android.app.Application
import android.app.ForegroundServiceStartNotAllowedException
import android.content.ComponentName
import android.content.ContextWrapper
import android.content.Intent
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeServiceControl
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Shadows.shadowOf

@RunWith(AndroidJUnit4::class)
class MissionRunnerTest {
    private val app: Application = ApplicationProvider.getApplicationContext()
    private val core = FakeCoreGateway()

    @Test
    fun open_starts_service() {
        shadowOf(app).grantPermissions(Manifest.permission.ACCESS_FINE_LOCATION)
        val runner = MissionRunner(core, MissionServiceControl(app))
        assertThat(runner.open("m1")).isEqualTo(Outcome.Ok(Unit))
        assertThat(core.calls).contains("openMission:m1")
        val started = shadowOf(app).nextStartedService
        assertThat(started.component).isEqualTo(ComponentName(app, MissionService::class.java))
        assertThat(started.action).isEqualTo(MissionService.ACTION_START)
        assertThat(runner.open.value).isEqualTo("m1")
        runner.close()
        assertThat(shadowOf(app).nextStoppedService.component).isEqualTo(ComponentName(app, MissionService::class.java))
        assertThat(runner.open.value).isNull()
        assertThat(core.calls.last()).isEqualTo("closeMission")
    }

    @Test
    fun a_refused_mission_starts_nothing() {
        val service = FakeServiceControl()
        core.outcomes["openMission"] = Outcome.Failed(CoreException.NoMission())
        val runner = MissionRunner(core, service)
        assertThat(runner.open("m1")).isInstanceOf(Outcome.Failed::class.java)
        assertThat(service.starts).isEqualTo(0)
        assertThat(runner.open.value).isNull()
    }

    @Test
    fun service_failure_visible() {
        val runner = MissionRunner(core, FakeServiceControl())
        runner.serviceFailed("x")
        assertThat(runner.background.value).isEqualTo(BackgroundState.Off("x"))
        runner.close()
        assertThat(runner.background.value).isEqualTo(BackgroundState.Idle)
        runner.serviceStarted()
        assertThat(runner.background.value).isEqualTo(BackgroundState.Running)
        runner.serviceStopped()
        assertThat(runner.background.value).isEqualTo(BackgroundState.Idle)
    }

    @Test
    fun a_quick_reopen_starts_the_service_again() {
        val service = FakeServiceControl()
        val runner = MissionRunner(core, service)
        runner.open("m1")
        runner.serviceStarted()
        runner.open("m2")
        assertThat(service.starts).isEqualTo(1)
        runner.close()
        runner.open("m3")
        assertThat(service.starts).isEqualTo(2)
        assertThat(service.stops).isEqualTo(1)
        runner.serviceStopped()
        runner.serviceStarted()
        assertThat(runner.background.value).isEqualTo(BackgroundState.Running)
    }

    @Test
    fun missing_location_turns_background_off() {
        val runner = MissionRunner(core, MissionServiceControl(app))
        runner.open("m1")
        assertThat(runner.background.value).isEqualTo(BackgroundState.Off("Location off"))
        assertThat(runner.open.value).isEqualTo("m1")
    }

    @Test
    fun openSurvivesARefusedForegroundService() {
        shadowOf(app).grantPermissions(Manifest.permission.ACCESS_FINE_LOCATION)
        val refusing =
            object : ContextWrapper(app) {
                override fun startForegroundService(service: Intent): ComponentName = throw ForegroundServiceStartNotAllowedException("in background")
            }
        val runner = MissionRunner(core, MissionServiceControl(refusing))
        assertThat(runner.open("m1")).isEqualTo(Outcome.Ok(Unit))
        assertThat(runner.open.value).isEqualTo("m1")
        assertThat(runner.background.value).isEqualTo(BackgroundState.Off("in background"))
    }
}
