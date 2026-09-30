package dev.newspicel.sdrmm.sensors

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationManager
import android.os.Build
import android.os.Handler
import android.os.SystemClock
import androidx.core.content.ContextCompat
import androidx.core.location.LocationListenerCompat
import androidx.core.location.LocationManagerCompat
import androidx.core.location.LocationRequestCompat
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.settings.SettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.Executor

class LocationSource(
    private val context: Context,
    private val handler: Handler,
    private val core: CoreGateway,
    private val settings: SettingsStore,
    private val declination: Declination,
    private val scope: CoroutineScope,
    private val fix: MutableStateFlow<LocationSample?>,
    private val status: ((SensorStatus) -> SensorStatus) -> Unit,
) : LocationListenerCompat {
    private val manager = context.getSystemService(LocationManager::class.java)
    private val executor = Executor { command -> handler.post(command) }
    private val noFix = Runnable { status { it.copy(fix = false) } }
    private var registered = false
    private var savedAtMs = 0L

    fun start() {
        handler.post(::register)
        handler.post(::seedDeclination)
    }

    fun restart() {
        handler.post {
            unregister()
            register()
        }
    }

    fun stop() {
        handler.post(::unregister)
    }

    private fun register() {
        val fine = granted(Manifest.permission.ACCESS_FINE_LOCATION)
        val coarse = granted(Manifest.permission.ACCESS_COARSE_LOCATION)
        val enabled = manager?.isProviderEnabled(LocationManager.GPS_PROVIDER) == true
        status {
            it.copy(
                access = if (fine || coarse) LocationAccess.WhileUsing else LocationAccess.Off,
                precise = fine,
                gpsEnabled = enabled,
                lastError = if (manager == null) "No location service" else null,
            )
        }
        if (manager == null || !(fine || coarse)) return
        try {
            LocationManagerCompat.requestLocationUpdates(manager, provider(fine), request(), executor, this)
            registered = true
            handler.postDelayed(noFix, NO_FIX_MS)
        } catch (error: SecurityException) {
            status { it.copy(access = LocationAccess.Off, lastError = error.message) }
        } catch (error: IllegalArgumentException) {
            status { it.copy(lastError = error.message) }
        }
    }

    private fun unregister() {
        handler.removeCallbacks(noFix)
        if (registered && manager != null) {
            try {
                LocationManagerCompat.removeUpdates(manager, this)
            } catch (error: SecurityException) {
                status { it.copy(lastError = error.message) }
            }
        }
        registered = false
    }

    private fun provider(fine: Boolean): String = if (fine || Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) LocationManager.GPS_PROVIDER else LocationManager.NETWORK_PROVIDER

    private fun request(): LocationRequestCompat = LocationRequestCompat
        .Builder(INTERVAL_MS)
        .setQuality(LocationRequestCompat.QUALITY_HIGH_ACCURACY)
        .setMinUpdateIntervalMillis(MIN_INTERVAL_MS)
        .build()

    override fun onLocationChanged(location: Location) {
        val sample = SampleMapping.location(location, SystemClock.elapsedRealtimeNanos(), System.currentTimeMillis()) ?: return
        core.pushLocation(sample)
        declination.update(sample.lat, sample.lon, sample.altM ?: 0.0)
        fix.value = sample
        handler.removeCallbacks(noFix)
        handler.postDelayed(noFix, NO_FIX_MS)
        status { it.copy(fix = true, accuracyM = location.accuracy) }
        persist(sample)
    }

    override fun onProviderEnabled(provider: String) {
        if (provider == LocationManager.GPS_PROVIDER) status { it.copy(gpsEnabled = true) }
    }

    override fun onProviderDisabled(provider: String) {
        if (provider == LocationManager.GPS_PROVIDER) status { it.copy(gpsEnabled = false, fix = false) }
    }

    private fun persist(sample: LocationSample) {
        val now = SystemClock.elapsedRealtime()
        if (savedAtMs != 0L && now - savedAtMs < SAVE_EVERY_MS) return
        savedAtMs = now
        scope.launch { settings.update { it.copy(lastFix = LatLon(sample.lat, sample.lon)) } }
    }

    private fun seedDeclination() {
        val saved = settings.settings.value.lastFix
        if (saved != null) {
            declination.update(saved.lat, saved.lon, 0.0)
            return
        }
        val last = lastKnown() ?: return
        declination.update(last.latitude, last.longitude, if (last.hasAltitude()) last.altitude else 0.0)
    }

    private fun lastKnown(): Location? {
        if (manager == null) return null
        if (!granted(Manifest.permission.ACCESS_FINE_LOCATION) && !granted(Manifest.permission.ACCESS_COARSE_LOCATION)) {
            return null
        }
        val providers =
            buildList {
                add(LocationManager.GPS_PROVIDER)
                add(LocationManager.NETWORK_PROVIDER)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) add(LocationManager.FUSED_PROVIDER)
            }
        return try {
            providers.firstNotNullOfOrNull { manager.getLastKnownLocation(it) }
        } catch (error: SecurityException) {
            status { it.copy(lastError = error.message) }
            null
        } catch (error: IllegalArgumentException) {
            status { it.copy(lastError = error.message) }
            null
        }
    }

    private fun granted(permission: String): Boolean = ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED

    private companion object {
        const val INTERVAL_MS = 1_000L
        const val MIN_INTERVAL_MS = 500L
        const val NO_FIX_MS = 5_000L
        const val SAVE_EVERY_MS = 60_000L
    }
}
