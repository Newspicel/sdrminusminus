package dev.newspicel.sdrmm.sensors

import android.content.Context
import android.os.Handler
import android.os.HandlerThread
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.settings.SettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

enum class Holder { App, Service, Car }

enum class LocationAccess { Off, WhileUsing }

data class SensorStatus(
    val access: LocationAccess,
    val precise: Boolean,
    val gpsEnabled: Boolean,
    val fix: Boolean,
    val accuracyM: Float?,
    val compass: Boolean,
    val compassReliable: Boolean,
    val gyro: Boolean,
    val lastError: String?,
) {
    companion object {
        val Unknown =
            SensorStatus(
                access = LocationAccess.Off,
                precise = false,
                gpsEnabled = true,
                fix = false,
                accuracyM = null,
                compass = true,
                compassReliable = true,
                gyro = false,
                lastError = null,
            )
    }
}

class SensorHub(
    private val context: Context,
    private val core: CoreGateway,
    private val settings: SettingsStore,
    private val declination: Declination,
    private val scope: CoroutineScope,
) {
    private val state = MutableStateFlow(SensorStatus.Unknown)
    private val fix = MutableStateFlow<LocationSample?>(null)
    private val holders = mutableSetOf<Holder>()
    private var thread: HandlerThread? = null
    private var location: LocationSource? = null
    private var orientation: OrientationSource? = null

    val status: StateFlow<SensorStatus> = state.asStateFlow()
    val lastFix: StateFlow<LocationSample?> = fix.asStateFlow()

    @Synchronized
    fun acquire(holder: Holder) {
        val first = holders.isEmpty()
        holders += holder
        if (first) start()
    }

    @Synchronized
    fun release(holder: Holder) {
        if (!holders.remove(holder) || holders.isNotEmpty()) return
        stop()
    }

    @Synchronized
    fun permissionsChanged() {
        location?.restart()
    }

    private fun start() {
        val started = HandlerThread(THREAD_NAME).apply { start() }
        val handler = Handler(started.looper)
        thread = started
        location =
            LocationSource(context, handler, core, settings, declination, scope, fix, ::update).also { it.start() }
        orientation = OrientationSource(context, handler, core, declination, ::update).also { it.start() }
    }

    private fun stop() {
        location?.stop()
        orientation?.stop()
        location = null
        orientation = null
        thread?.quitSafely()
        thread = null
    }

    private fun update(transform: (SensorStatus) -> SensorStatus) {
        state.update(transform)
    }

    private companion object {
        const val THREAD_NAME = "sdrmm-sensors"
    }
}
