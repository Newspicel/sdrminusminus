package dev.newspicel.sdrmm.mission

import android.Manifest
import android.app.Application
import android.app.NotificationManager
import android.content.Intent
import androidx.car.app.notification.CarAppExtender
import androidx.core.app.NotificationCompat
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.nav.NavIntents
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Shadows.shadowOf

@RunWith(AndroidJUnit4::class)
class AlertNotifierTest {
    private val app: Application = ApplicationProvider.getApplicationContext()
    private val notifier = AlertNotifier(app, FakeCoreGateway())
    private val manager = app.getSystemService(NotificationManager::class.java)

    @Before
    fun allow() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        MissionNotifications.createChannels(app)
    }

    @Test
    fun retarget() {
        val nav = NavIntents.view("geo:52.530000,13.420000").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        assertThat(notifier.retarget(Samples.notice(RetargetReason.MOVED, kind = GuidanceKind.ESTIMATE), "1.2 km", nav, car = false)).isTrue()
        val posted = shadowOf(manager).allNotifications.single()
        assertThat(posted.channelId).isEqualTo(MissionNotifications.CHANNEL_ALERTS)
        assertThat(posted.extras.getCharSequence(NotificationCompat.EXTRA_TITLE)).isEqualTo("New target")
        assertThat(posted.extras.getCharSequence(NotificationCompat.EXTRA_TEXT)).isEqualTo("1.2 km · Target")
        assertThat(posted.actions.single().title).isEqualTo("Navigate")
        assertThat(CarAppExtender(posted).actions).isEmpty()
    }

    @Test
    fun car_extender() {
        val nav = NavIntents.view("geo:52.530000,13.420000")
        notifier.retarget(Samples.notice(RetargetReason.MOVED, kind = GuidanceKind.PROBE), "900 m", nav, car = true)
        val posted = shadowOf(manager).allNotifications.single()
        assertThat(posted.extras.getCharSequence(NotificationCompat.EXTRA_TEXT)).isEqualTo("900 m · Cross")
        val extender = CarAppExtender(posted)
        assertThat(extender.actions.single().title).isEqualTo("Navigate")
        assertThat(extender.importance).isEqualTo(NotificationManager.IMPORTANCE_HIGH)
    }

    @Test
    fun denied_notifications_post_nothing() {
        shadowOf(app).denyPermissions(Manifest.permission.POST_NOTIFICATIONS)
        assertThat(notifier.enabled).isFalse()
        assertThat(notifier.retarget(Samples.notice(), "1 km", NavIntents.view("geo:0,0"), car = false)).isFalse()
        assertThat(shadowOf(manager).allNotifications).isEmpty()
    }
}
