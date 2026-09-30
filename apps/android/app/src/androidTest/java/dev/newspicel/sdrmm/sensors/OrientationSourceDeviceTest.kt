package dev.newspicel.sdrmm.sensors

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorManager
import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OrientationSourceDeviceTest {
    @get:Rule val foreground = ActivityScenarioRule(MainActivity::class.java)

    private val context: Context = ApplicationProvider.getApplicationContext()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val core = FakeCoreGateway()
    private val hub = SensorHub(context, core, FakeSettingsStore(), Declination(GeomagneticDeclination(), System::currentTimeMillis), scope)

    @After
    fun cleanUp() {
        hub.release(Holder.App)
        scope.cancel()
    }

    @Test
    fun rotation_vector_gives_headings_or_says_no_compass() {
        val rotation = context.getSystemService(SensorManager::class.java)?.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR)
        hub.acquire(Holder.App)
        if (rotation == null) {
            waitFor { !hub.status.value.compass }
            assertThat(hub.status.value.compass).isFalse()
            return
        }
        waitFor { core.headings.isNotEmpty() }
        val heading = core.headings.first()
        assertThat(heading.magneticDeg).isAtLeast(0.0)
        assertThat(heading.magneticDeg).isLessThan(360.0)
        assertThat(hub.status.value.compass).isTrue()
    }

    private fun waitFor(condition: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (!condition() && SystemClock.elapsedRealtime() < deadline) Thread.sleep(STEP_MS)
    }

    private companion object {
        const val PATIENCE_MS = 2_000L
        const val STEP_MS = 50L
    }
}
