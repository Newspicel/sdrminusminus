package dev.newspicel.sdrmm.nav

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.annotation.StringRes
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.NavApp
import dev.newspicel.sdrmm.settings.NavChoice
import dev.newspicel.sdrmm.settings.SettingsStore
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

sealed interface HandoffResult {
    data object Opened : HandoffResult

    data object FellBack : HandoffResult

    data class Failed(
        @param:StringRes val reason: Int,
    ) : HandoffResult
}

data class Handoff(
    val intent: Intent,
    val fellBack: Boolean,
    val resolves: Boolean,
)

class NavHandoff(
    private val context: Context,
    private val core: CoreGateway,
    private val settings: SettingsStore,
) {
    private val missions = MutableStateFlow<Set<String>>(emptySet())
    val handedOff: StateFlow<Set<String>> = missions.asStateFlow()

    fun intentFor(target: LatLon): Handoff {
        val choice = settings.settings.value.navChoice ?: if (mapsInstalled(target)) NavChoice.GoogleMaps else NavChoice.Ask
        if (choice == NavChoice.GoogleMaps) {
            val maps = NavIntents.googleMaps(core.navUri(target, NavApp.GOOGLE_MAPS))
            if (resolves(maps)) return Handoff(maps.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), fellBack = false, resolves = true)
        }
        val uri = core.navUri(target, NavApp.CHOOSER)
        val chooser = NavIntents.chooser(uri, context.getString(R.string.navigate)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return Handoff(chooser, fellBack = choice == NavChoice.GoogleMaps, resolves = resolves(NavIntents.view(uri)))
    }

    fun open(
        activity: Activity,
        mission: String,
        target: LatLon,
    ): HandoffResult {
        val handoff = intentFor(target)
        if (!handoff.resolves) return HandoffResult.Failed(R.string.no_map_app)
        try {
            activity.startActivity(handoff.intent)
        } catch (_: ActivityNotFoundException) {
            return HandoffResult.Failed(R.string.no_map_app)
        }
        handedOff(mission)
        return if (handoff.fellBack) HandoffResult.FellBack else HandoffResult.Opened
    }

    fun handedOff(mission: String) {
        missions.update { it + mission }
    }

    private fun mapsInstalled(target: LatLon): Boolean = resolves(NavIntents.googleMaps(core.navUri(target, NavApp.GOOGLE_MAPS)))

    private fun resolves(intent: Intent): Boolean = context.packageManager
        .queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
        .isNotEmpty()
}
