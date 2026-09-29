package dev.newspicel.sdrmm.mission

import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.car.app.notification.CarAppExtender
import androidx.car.app.notification.CarPendingIntent
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.NavApp
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.nav.NavIntents
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionPlan

class AlertNotifier(
    private val context: Context,
    private val core: CoreGateway,
) {
    val enabled: Boolean
        get() = PermissionPlan.granted(context, Need.Notifications) && NotificationManagerCompat.from(context).areNotificationsEnabled()

    fun retarget(
        notice: RetargetNotice,
        distanceText: String,
        navIntent: Intent,
        car: Boolean,
    ): Boolean {
        if (!enabled) return false
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val navigate = PendingIntent.getActivity(context, MissionNotifications.RETARGET_ID, navIntent, flags)
        val label = context.getString(R.string.navigate)
        val kind = context.getString(if (notice.target.kind == GuidanceKind.PROBE) R.string.target_cross else R.string.target_estimate)
        val builder =
            NotificationCompat
                .Builder(context, MissionNotifications.CHANNEL_ALERTS)
                .setSmallIcon(R.drawable.ic_navigate)
                .setContentTitle(context.getString(R.string.new_target))
                .setContentText(context.getString(R.string.retarget_text, distanceText, kind))
                .setCategory(NotificationCompat.CATEGORY_NAVIGATION)
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setAutoCancel(true)
                .setContentIntent(navigate)
                .addAction(R.drawable.ic_navigate, label, navigate)
        if (car) builder.extend(carExtender(notice, label))
        return try {
            NotificationManagerCompat.from(context).notify(notice.mission, MissionNotifications.RETARGET_ID, builder.build())
            true
        } catch (_: SecurityException) {
            false
        }
    }

    private fun carExtender(
        notice: RetargetNotice,
        label: String,
    ): CarAppExtender {
        val intent = NavIntents.car(core.navUri(notice.target.at, NavApp.CAR))
        val car = CarPendingIntent.getCarApp(context, MissionNotifications.RETARGET_ID, intent, PendingIntent.FLAG_UPDATE_CURRENT)
        return CarAppExtender
            .Builder()
            .setContentIntent(car)
            .addAction(R.drawable.ic_navigate, label, car)
            .setImportance(NotificationManager.IMPORTANCE_HIGH)
            .build()
    }
}
