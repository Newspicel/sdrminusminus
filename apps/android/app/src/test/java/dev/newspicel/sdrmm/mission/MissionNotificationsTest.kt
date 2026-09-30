package dev.newspicel.sdrmm.mission

import android.app.Application
import android.app.Notification
import android.app.NotificationManager
import androidx.core.app.NotificationCompat
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MissionNotificationsTest {
    private val app: Application = ApplicationProvider.getApplicationContext()

    @Test
    fun ongoing() {
        MissionNotifications.createChannels(app)
        val notification = MissionNotifications.ongoing(app, "Kraken DF", "Online", "d1")
        assertThat(notification.channelId).isEqualTo(MissionNotifications.CHANNEL_MISSION)
        assertThat(notification.flags and Notification.FLAG_ONGOING_EVENT).isNotEqualTo(0)
        assertThat(notification.extras.getCharSequence(NotificationCompat.EXTRA_TITLE)).isEqualTo("Kraken DF")
        assertThat(notification.extras.getCharSequence(NotificationCompat.EXTRA_TEXT)).isEqualTo("Online")
        assertThat(notification.actions.single().title).isEqualTo("Stop")
        val manager = app.getSystemService(NotificationManager::class.java)
        assertThat(manager.getNotificationChannel(MissionNotifications.CHANNEL_MISSION).importance).isEqualTo(NotificationManager.IMPORTANCE_LOW)
        assertThat(manager.getNotificationChannel(MissionNotifications.CHANNEL_ALERTS).importance).isEqualTo(NotificationManager.IMPORTANCE_HIGH)
    }
}
