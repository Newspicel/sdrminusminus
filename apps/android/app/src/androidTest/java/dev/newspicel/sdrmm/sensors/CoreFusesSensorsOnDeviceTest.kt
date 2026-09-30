package dev.newspicel.sdrmm.sensors

import android.Manifest
import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorManager
import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.rule.GrantPermissionRule
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.RealCore
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.core.UniffiCoreGateway
import dev.newspicel.sdrmm.ffi.HeadingSourceKind
import dev.newspicel.sdrmm.secrets.KeystoreCipher
import dev.newspicel.sdrmm.secrets.KeystoreVault
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.After
import org.junit.Assume.assumeNotNull
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RealCore
@RunWith(AndroidJUnit4::class)
class CoreFusesSensorsOnDeviceTest {
    @get:Rule(order = 0)
    val permissions: GrantPermissionRule =
        GrantPermissionRule.grant(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)

    @get:Rule(order = 1)
    val foreground = ActivityScenarioRule(MainActivity::class.java)

    private val context: Context = ApplicationProvider.getApplicationContext()
    private val dir = File(context.noBackupFilesDir, "vault-pose-test")
    private val gps = MockGps(context)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val core = (UniffiCoreGateway.create(context, KeystoreVault(dir, KeystoreCipher(ALIAS))) as Outcome.Ok).value
    private val hub = SensorHub(context, core, FakeSettingsStore(), Declination(GeomagneticDeclination(), System::currentTimeMillis), scope)

    @Before
    fun start() {
        gps.start()
        core.run(scope)
    }

    @After
    fun cleanUp() {
        hub.release(Holder.App)
        gps.stop()
        scope.cancel()
        dir.deleteRecursively()
    }

    @Test
    fun the_phone_orientation_becomes_a_compass_heading() {
        assumeNotNull(context.getSystemService(SensorManager::class.java)?.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR))
        hub.acquire(Holder.App)
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (core.pose.value?.headingDeg == null && SystemClock.elapsedRealtime() < deadline) {
            gps.push(LAT, LON)
            Thread.sleep(STEP_MS)
        }
        val pose = core.pose.value
        assertThat(pose?.headingDeg).isNotNull()
        assertThat(pose?.source).isAnyOf(HeadingSourceKind.COMPASS, HeadingSourceKind.FUSED)
    }

    private companion object {
        const val ALIAS = "sdrmm.test.pose"
        const val LAT = 52.52
        const val LON = 13.405
        const val PATIENCE_MS = 10_000L
        const val STEP_MS = 200L
    }
}
