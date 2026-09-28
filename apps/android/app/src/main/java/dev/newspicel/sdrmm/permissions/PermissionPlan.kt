package dev.newspicel.sdrmm.permissions

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat

enum class Need { Camera, LocalNetwork, Location, Notifications }

object PermissionPlan {
    const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
    const val LOCAL_NETWORK_SDK = 37
    private const val NOTIFICATIONS_SDK = 33

    fun permissions(
        need: Need,
        sdk: Int,
    ): List<String> = when (need) {
        Need.Camera -> listOf(Manifest.permission.CAMERA)
        Need.LocalNetwork -> if (sdk >= LOCAL_NETWORK_SDK) listOf(LOCAL_NETWORK) else emptyList()
        Need.Location -> listOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)
        Need.Notifications -> if (sdk >= NOTIFICATIONS_SDK) listOf(NOTIFICATION_PERMISSION) else emptyList()
    }

    fun missing(
        need: Need,
        sdk: Int,
        granted: Set<String>,
    ): List<String> = permissions(need, sdk).filterNot { it in granted }

    fun granted(
        context: Context,
        need: Need,
    ): Boolean = permissions(need, Build.VERSION.SDK_INT).all {
        ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
    }

    private const val NOTIFICATION_PERMISSION = "android.permission.POST_NOTIFICATIONS"
}
