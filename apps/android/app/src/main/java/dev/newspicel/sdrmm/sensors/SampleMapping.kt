package dev.newspicel.sdrmm.sensors

import android.hardware.SensorManager
import android.location.Location
import dev.newspicel.sdrmm.ffi.HeadingSample
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.MagAccuracy
import dev.newspicel.sdrmm.ffi.MotionFrame
import dev.newspicel.sdrmm.ffi.MotionSample

object SampleMapping {
    private const val HIGH_DEG = 10.0
    private const val MEDIUM_DEG = 20.0
    private const val LOW_DEG = 35.0
    private const val ACCURACY_INDEX = 4
    private const val NANOS_PER_MILLI = 1_000_000L

    fun location(location: Location): LocationSample? {
        if (!location.hasAccuracy()) return null
        return LocationSample(
            tUnixMs = location.time,
            lat = location.latitude,
            lon = location.longitude,
            altM = if (location.hasAltitude()) location.altitude else null,
            hAccM = location.accuracy.toDouble(),
            vAccM = if (location.hasVerticalAccuracy()) location.verticalAccuracyMeters.toDouble() else null,
            speedMps = if (location.hasSpeed()) location.speed.toDouble() else null,
            speedAccMps = if (location.hasSpeedAccuracy()) location.speedAccuracyMetersPerSecond.toDouble() else null,
            courseDeg = if (location.hasBearing()) location.bearing.toDouble() else null,
            courseAccDeg = if (location.hasBearingAccuracy()) location.bearingAccuracyDegrees.toDouble() else null,
        )
    }

    fun heading(
        r: DoubleArray,
        declinationDeg: Double?,
        accuracyDeg: Double?,
        unixMs: Long,
    ): HeadingSample {
        val magnetic = OrientationMath.magneticHeadingDeg(r)
        return HeadingSample(
            tUnixMs = unixMs,
            trueDeg = declinationDeg?.let { OrientationMath.wrap360(magnetic + it) },
            magneticDeg = magnetic,
            accuracyDeg = accuracyDeg,
        )
    }

    fun motion(
        q: DoubleArray,
        r: DoubleArray,
        gyro: FloatArray,
        heading: HeadingSample,
        status: Int,
        unixMs: Long,
    ): MotionSample = MotionSample(
        tUnixMs = unixMs,
        frame = MotionFrame.ENU_MAGNETIC,
        qw = q[0],
        qx = q[1],
        qy = q[2],
        qz = q[3],
        rotX = gyro[0].toDouble(),
        rotY = gyro[1].toDouble(),
        rotZ = gyro[2].toDouble(),
        gravX = -r[6],
        gravY = -r[7],
        gravZ = -r[8],
        headingDeg = heading.trueDeg,
        magAccuracy = magAccuracy(status),
    )

    fun accuracyDeg(
        values: FloatArray,
        status: Int,
    ): Double? {
        if (values.size > ACCURACY_INDEX && values[ACCURACY_INDEX] > 0f) {
            return Math.toDegrees(values[ACCURACY_INDEX].toDouble())
        }
        return when (status) {
            SensorManager.SENSOR_STATUS_ACCURACY_HIGH -> HIGH_DEG
            SensorManager.SENSOR_STATUS_ACCURACY_MEDIUM -> MEDIUM_DEG
            SensorManager.SENSOR_STATUS_ACCURACY_LOW -> LOW_DEG
            else -> null
        }
    }

    fun magAccuracy(status: Int): MagAccuracy = when (status) {
        SensorManager.SENSOR_STATUS_ACCURACY_HIGH -> MagAccuracy.HIGH
        SensorManager.SENSOR_STATUS_ACCURACY_MEDIUM -> MagAccuracy.MEDIUM
        SensorManager.SENSOR_STATUS_ACCURACY_LOW -> MagAccuracy.LOW
        else -> MagAccuracy.UNCALIBRATED
    }

    fun unixMillis(
        eventNanos: Long,
        nowElapsedNanos: Long,
        nowUnixMs: Long,
    ): Long = nowUnixMs - (nowElapsedNanos - eventNanos) / NANOS_PER_MILLI
}
