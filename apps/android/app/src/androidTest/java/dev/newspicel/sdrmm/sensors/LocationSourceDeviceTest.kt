package dev.newspicel.sdrmm.sensors

import android.Manifest
import android.content.Context
import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LocationSourceDeviceTest {
    @get:Rule(order = 0)
    val permissions: GrantPermissionRule =
        GrantPermissionRule.grant(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)

    @get:Rule(order = 1)
    val foreground = ActivityScenarioRule(MainActivity::class.java)

    private val context: Context = ApplicationProvider.getApplicationContext()
    private val gps = MockGps(context)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val core = FakeCoreGateway()
    private val hub = SensorHub(context, core, FakeSettingsStore(), Declination(GeomagneticDeclination(), System::currentTimeMillis), scope)

    @Before
    fun mockProvider() {
        gps.start()
    }

    @After
    fun cleanUp() {
        hub.release(Holder.App)
        gps.stop()
        scope.cancel()
    }

    @Test
    fun a_mock_fix_reaches_the_core() {
        hub.acquire(Holder.App)
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (core.locations.isEmpty() && SystemClock.elapsedRealtime() < deadline) {
            gps.push(LAT, LON)
            Thread.sleep(STEP_MS)
        }
        assertThat(core.locations).isNotEmpty()
        val sample = core.locations.first()
        assertThat(sample.lat).isWithin(1e-6).of(LAT)
        assertThat(sample.lon).isWithin(1e-6).of(LON)
        assertThat(sample.hAccM).isEqualTo(MockGps.ACCURACY_M.toDouble())
        assertThat(hub.status.value.fix).isTrue()
        assertThat(hub.status.value.access).isEqualTo(LocationAccess.WhileUsing)
        assertThat(hub.lastFix.value?.lat).isWithin(1e-6).of(LAT)
    }

    private companion object {
        const val LAT = 52.52
        const val LON = 13.405
        const val PATIENCE_MS = 3_000L
        const val STEP_MS = 200L
    }
}
