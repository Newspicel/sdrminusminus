package dev.newspicel.sdrmm.permissions

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat

class PermissionGate(
    private val context: Context,
    private val denials: MutableMap<Need, Int>,
    private val report: (Need, Boolean) -> Unit,
) {
    private val queue = ArrayDeque<Need>()
    var launch: (Need, Array<String>) -> Unit = { need, _ -> answered(need, false) }

    fun granted(need: Need): Boolean = PermissionPlan.granted(context, need)

    fun request(
        need: Need,
        fromUser: Boolean = true,
    ) {
        val missing = PermissionPlan.missing(need, Build.VERSION.SDK_INT, grantedSet(need))
        when {
            missing.isEmpty() -> answered(need, true)
            (denials[need] ?: 0) < MAX_ASKS -> launch(need, missing.toTypedArray())
            fromUser -> openAppSettings(context)
            else -> answered(need, false)
        }
    }

    fun requestInOrder(needs: List<Need>) {
        queue.clear()
        queue.addAll(needs)
        next()
    }

    fun answered(
        need: Need,
        allowed: Boolean,
    ) {
        if (!allowed) denials[need] = (denials[need] ?: 0) + 1
        report(need, allowed)
        next()
    }

    private fun next() {
        val need = queue.removeFirstOrNull() ?: return
        request(need, fromUser = false)
    }

    private fun grantedSet(need: Need): Set<String> = PermissionPlan
        .permissions(need, Build.VERSION.SDK_INT)
        .filter { ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED }
        .toSet()

    companion object {
        private const val MAX_ASKS = 2
        private const val TAG = "SdrmmPermissions"

        fun openAppSettings(context: Context) {
            open(context, Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)))
        }

        fun openLocationSettings(context: Context) {
            open(context, Intent(Settings.ACTION_LOCATION_SOURCE_SETTINGS))
        }

        private fun open(
            context: Context,
            intent: Intent,
        ) {
            try {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            } catch (error: ActivityNotFoundException) {
                Log.w(TAG, "No settings screen for ${intent.action}", error)
            }
        }
    }
}

@Composable
fun rememberPermissionGate(onResult: (Need, Boolean) -> Unit): PermissionGate {
    val context = LocalContext.current
    val latest by rememberUpdatedState(onResult)
    val denials = remember { mutableStateMapOf<Need, Int>() }
    var asking by rememberSaveable { mutableStateOf<Need?>(null) }
    val gate = remember(context) { PermissionGate(context, denials) { need, allowed -> latest(need, allowed) } }
    val launcher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { results ->
            val need = asking ?: return@rememberLauncherForActivityResult
            asking = null
            gate.answered(need, results.values.any { it })
        }
    gate.launch = { need, permissions ->
        asking = need
        launcher.launch(permissions)
    }
    return gate
}
