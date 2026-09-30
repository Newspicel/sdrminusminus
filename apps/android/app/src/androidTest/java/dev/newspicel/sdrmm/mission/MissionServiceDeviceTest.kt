package dev.newspicel.sdrmm.mission

import android.Manifest
import android.app.NotificationManager
import android.content.ContextWrapper
import android.content.pm.PackageManager
import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.Startup
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.TestSdrmmApp
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MissionServiceDeviceTest {
    @get:Rule(order = 0)
    val permissions: GrantPermissionRule =
        GrantPermissionRule.grant(
            Manifest.permission.ACCESS_FINE_LOCATION,
            Manifest.permission.ACCESS_COARSE_LOCATION,
            Manifest.permission.POST_NOTIFICATIONS,
        )

    @get:Rule(order = 1)
    val foreground = ActivityScenarioRule(MainActivity::class.java)

    private val app: TestSdrmmApp = ApplicationProvider.getApplicationContext()
    private val graph get() = (app.startup.value as Startup.Ready).graph

    @After
    fun close() {
        InstrumentationRegistry.getInstrumentation().runOnMainSync { graph.runner.close() }
    }

    @Test
    fun foreground() {
        InstrumentationRegistry.getInstrumentation().runOnMainSync { graph.runner.open("m1") }
        val manager = app.getSystemService(NotificationManager::class.java)
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (SystemClock.elapsedRealtime() < deadline && graph.runner.background.value != BackgroundState.Running) {
            Thread.sleep(STEP_MS)
        }
        assertThat(graph.runner.background.value).isEqualTo(BackgroundState.Running)
        val ongoing = manager.activeNotifications.firstOrNull { it.id == MissionNotifications.ONGOING_ID }
        assertThat(ongoing?.notification?.channelId).isEqualTo(MissionNotifications.CHANNEL_MISSION)
    }

    @Test
    fun denied_visible() {
        val denied =
            object : ContextWrapper(app) {
                override fun checkPermission(
                    permission: String,
                    pid: Int,
                    uid: Int,
                ): Int = PackageManager.PERMISSION_DENIED
            }
        val runner = MissionRunner(FakeCoreGateway(), MissionServiceControl(denied))
        runner.open("m1")
        assertThat(runner.background.value).isEqualTo(BackgroundState.Off("Location off"))
        assertThat(runner.open.value).isEqualTo("m1")
    }

    private companion object {
        const val PATIENCE_MS = 5_000L
        const val STEP_MS = 100L
    }
}
