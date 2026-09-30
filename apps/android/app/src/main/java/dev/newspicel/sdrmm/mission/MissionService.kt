package dev.newspicel.sdrmm.mission

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.SdrmmApp
import dev.newspicel.sdrmm.Startup
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.sensors.Holder
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch

class MissionService : LifecycleService() {
    private var graph: AppGraph? = null
    private var updates: Job? = null

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        super.onStartCommand(intent, flags, startId)
        val ready = ((application as? SdrmmApp)?.startup?.value as? Startup.Ready)?.graph
        if (ready == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (intent?.action == ACTION_STOP) {
            if (ready.runner.open.value != null) ready.runner.close()
            stopSelf()
            return START_NOT_STICKY
        }
        if (!foreground(ready)) return START_NOT_STICKY
        ready.runner.serviceStarted()
        if (graph == null) attach(ready)
        return START_NOT_STICKY
    }

    private fun foreground(ready: AppGraph): Boolean {
        val open = ready.runner.open.value
        val title = open?.let { id -> ready.missionTitle(id) } ?: getString(R.string.channel_mission)
        val notification = MissionNotifications.ongoing(this, title, linkLabel(ready.core.link.value), open)
        val refusal =
            try {
                ServiceCompat.startForeground(this, MissionNotifications.ONGOING_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION)
                null
            } catch (error: IllegalStateException) {
                error.message ?: error.javaClass.simpleName
            } catch (error: SecurityException) {
                error.message ?: error.javaClass.simpleName
            }
        refusal ?: return true
        ready.runner.serviceFailed(refusal)
        stopSelf()
        return false
    }

    private fun attach(ready: AppGraph) {
        graph = ready
        ready.sensors.acquire(Holder.Service)
        updates = lifecycleScope.launch { follow(ready) }
    }

    private suspend fun follow(ready: AppGraph) {
        combine(ready.runner.open, ready.core.missions, ready.core.link) { open, _, link ->
            Triple(open, open?.let { ready.missionTitle(it) } ?: getString(R.string.channel_mission), linkLabel(link))
        }.distinctUntilChanged().collect { (open, title, link) ->
            try {
                NotificationManagerCompat.from(this).notify(MissionNotifications.ONGOING_ID, MissionNotifications.ongoing(this, title, link, open))
            } catch (error: SecurityException) {
                Log.w(TAG, "Mission notification not updated", error)
            }
        }
    }

    private fun linkLabel(link: LinkState): String = getString(
        when (link) {
            is LinkState.Online -> R.string.link_online
            is LinkState.Connecting -> R.string.link_connecting
            is LinkState.Refused -> R.string.link_refused
            LinkState.Offline -> R.string.link_offline
        },
    )

    override fun onDestroy() {
        updates?.cancel()
        graph?.let {
            it.sensors.release(Holder.Service)
            it.runner.serviceStopped()
        }
        graph = null
        super.onDestroy()
    }

    companion object {
        private const val TAG = "SdrmmMission"
        const val ACTION_START = "dev.newspicel.sdrmm.mission.START"
        const val ACTION_STOP = "dev.newspicel.sdrmm.mission.STOP"

        fun startIntent(context: Context): Intent = Intent(context, MissionService::class.java).setAction(ACTION_START)

        fun stopIntent(context: Context): Intent = Intent(context, MissionService::class.java).setAction(ACTION_STOP)
    }
}

class MissionServiceControl(
    private val context: Context,
) : ServiceControl {
    override fun start(): String? {
        val location = ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED
        if (!location) return context.getString(R.string.chip_location_off)
        return try {
            ContextCompat.startForegroundService(context, MissionService.startIntent(context))
            null
        } catch (error: IllegalStateException) {
            error.message ?: error.javaClass.simpleName
        } catch (error: SecurityException) {
            error.message ?: error.javaClass.simpleName
        }
    }

    override fun stop() {
        context.stopService(Intent(context, MissionService::class.java))
    }
}
