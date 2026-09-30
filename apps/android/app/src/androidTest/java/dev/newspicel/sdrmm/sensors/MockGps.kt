package dev.newspicel.sdrmm.sensors

import android.content.Context
import android.location.Location
import android.location.LocationManager
import android.location.provider.ProviderProperties
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry

class MockGps(
    private val context: Context,
) {
    private val manager = context.getSystemService(LocationManager::class.java)

    fun start() {
        shell("appops set ${context.packageName} android:mock_location allow")
        manager.addTestProvider(
            LocationManager.GPS_PROVIDER,
            ProviderProperties
                .Builder()
                .setHasSatelliteRequirement(true)
                .setAccuracy(ProviderProperties.ACCURACY_FINE)
                .setPowerUsage(ProviderProperties.POWER_USAGE_LOW)
                .build(),
        )
        manager.setTestProviderEnabled(LocationManager.GPS_PROVIDER, true)
    }

    fun push(
        lat: Double,
        lon: Double,
    ) {
        manager.setTestProviderLocation(
            LocationManager.GPS_PROVIDER,
            Location(LocationManager.GPS_PROVIDER).apply {
                latitude = lat
                longitude = lon
                accuracy = ACCURACY_M
                altitude = 34.0
                time = System.currentTimeMillis()
                elapsedRealtimeNanos = SystemClock.elapsedRealtimeNanos()
            },
        )
    }

    fun stop() {
        manager.removeTestProvider(LocationManager.GPS_PROVIDER)
    }

    private fun shell(command: String): String {
        val output =
            InstrumentationRegistry
                .getInstrumentation()
                .uiAutomation
                .executeShellCommand(command)
        return ParcelFileDescriptor.AutoCloseInputStream(output).use { it.readBytes().toString(Charsets.UTF_8) }
    }

    companion object {
        const val ACCURACY_M = 3f
    }
}
