package dev.newspicel.sdrmm.mission

import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.R

object MissionNotifications {
    const val CHANNEL_MISSION = "mission"
    const val CHANNEL_ALERTS = "alerts"
    const val ONGOING_ID = 1
    const val RETARGET_ID = 2
    const val EXTRA_MISSION = "mission"
    private const val OPEN_REQUEST = 1
    private const val STOP_REQUEST = 3

    fun createChannels(context: Context) {
        val manager = NotificationManagerCompat.from(context)
        manager.createNotificationChannel(
            NotificationChannelCompat
                .Builder(CHANNEL_MISSION, NotificationManagerCompat.IMPORTANCE_LOW)
                .setName(context.getString(R.string.channel_mission))
                .build(),
        )
        manager.createNotificationChannel(
            NotificationChannelCompat
                .Builder(CHANNEL_ALERTS, NotificationManagerCompat.IMPORTANCE_HIGH)
                .setName(context.getString(R.string.channel_alerts))
                .build(),
        )
    }

    fun ongoing(
        context: Context,
        title: String,
        text: String,
        missionId: String?,
    ): Notification {
        val open =
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .apply { missionId?.let { putExtra(EXTRA_MISSION, it) } }
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val stop = PendingIntent.getService(context, STOP_REQUEST, MissionService.stopIntent(context), flags)
        return NotificationCompat
            .Builder(context, CHANNEL_MISSION)
            .setSmallIcon(R.drawable.ic_mission)
            .setContentTitle(title)
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_NAVIGATION)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(PendingIntent.getActivity(context, OPEN_REQUEST, open, flags))
            .addAction(R.drawable.ic_close, context.getString(R.string.stop), stop)
            .build()
    }
}
