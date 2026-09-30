package dev.newspicel.sdrmm.sensors

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.SystemClock
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.ffi.HeadingSample

class OrientationSource(
    context: Context,
    private val handler: Handler,
    private val core: CoreGateway,
    private val declination: Declination,
    private val status: ((SensorStatus) -> SensorStatus) -> Unit,
) : SensorEventListener {
    private val manager = context.getSystemService(SensorManager::class.java)
    private val rotation = manager?.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR)
    private val gyroscope = manager?.getDefaultSensor(Sensor.TYPE_GYROSCOPE)
    private val q = DoubleArray(4)
    private val r = DoubleArray(9)
    private val gyro = FloatArray(3)
    private var heading: HeadingSample? = null
    private var events = 0L
    private var accuracy = SensorManager.SENSOR_STATUS_ACCURACY_HIGH

    fun start() {
        status { it.copy(compass = rotation != null, gyro = gyroscope != null) }
        if (manager == null || rotation == null) return
        manager.registerListener(this, rotation, PERIOD_US, handler)
        if (gyroscope != null) manager.registerListener(this, gyroscope, PERIOD_US, handler)
    }

    fun stop() {
        manager?.unregisterListener(this)
    }

    override fun onSensorChanged(event: SensorEvent) {
        when (event.sensor.type) {
            Sensor.TYPE_GYROSCOPE -> event.values.copyInto(gyro, endIndex = gyro.size)
            Sensor.TYPE_ROTATION_VECTOR -> rotated(event)
        }
    }

    private fun rotated(event: SensorEvent) {
        OrientationMath.quaternion(event.values, q)
        OrientationMath.matrix(q, r)
        val unixMs = SampleMapping.unixMillis(event.timestamp, SystemClock.elapsedRealtimeNanos(), System.currentTimeMillis())
        val latest =
            heading.takeIf { events % HEADING_EVERY != 0L }
                ?: SampleMapping
                    .heading(r, declination.current(), SampleMapping.accuracyDeg(event.values, accuracy), unixMs)
                    .also {
                        heading = it
                        core.pushHeading(it)
                    }
        events += 1
        if (gyroscope != null) core.pushMotion(SampleMapping.motion(q, r, gyro, latest, accuracy, unixMs))
    }

    override fun onAccuracyChanged(
        sensor: Sensor,
        accuracy: Int,
    ) {
        if (sensor.type != Sensor.TYPE_ROTATION_VECTOR) return
        this.accuracy = accuracy
        val reliable = accuracy != SensorManager.SENSOR_STATUS_UNRELIABLE && accuracy != SensorManager.SENSOR_STATUS_NO_CONTACT
        status { it.copy(compassReliable = reliable) }
    }

    private companion object {
        const val PERIOD_US = 20_000
        const val HEADING_EVERY = 5L
    }
}
