package dev.newspicel.sdrmm.ui.components

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionGate
import dev.newspicel.sdrmm.permissions.rememberPermissionGate

class MissionChipActions(
    val gate: PermissionGate,
    private val context: Context,
) {
    fun handle(action: ChipAction) {
        when (action) {
            ChipAction.AllowLocation -> gate.request(Need.Location)
            ChipAction.LocationSettings -> PermissionGate.openLocationSettings(context)
            ChipAction.AllowNotifications -> if (gate.granted(Need.Notifications)) openNotificationSettings() else gate.request(Need.Notifications)
        }
    }

    private fun openNotificationSettings() {
        val intent =
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            PermissionGate.openAppSettings(context)
        }
    }
}

@Composable
fun rememberMissionChipActions(
    graph: AppGraph,
    context: Context,
    onResult: (Need, Boolean) -> Unit = { _, _ -> },
): MissionChipActions {
    val gate =
        rememberPermissionGate { need, allowed ->
            graph.permissionsChanged()
            onResult(need, allowed)
        }
    return remember(gate, context) { MissionChipActions(gate, context) }
}
