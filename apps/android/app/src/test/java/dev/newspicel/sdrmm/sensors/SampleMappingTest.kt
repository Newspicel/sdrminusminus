package dev.newspicel.sdrmm.sensors

import android.hardware.SensorManager
import android.location.Location
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.MagAccuracy
import dev.newspicel.sdrmm.ffi.MotionFrame
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.sin
import kotlin.math.sqrt
import kotlin.random.Random

@RunWith(AndroidJUnit4::class)
class SampleMappingTest {
    @Test
    fun location_needs_accuracy() {
        val bare =
            Location("gps").apply {
                latitude = 52.52
                longitude = 13.405
                time = 1_000
            }
        assertThat(SampleMapping.location(bare, NOW_ELAPSED_NS, NOW_UNIX_MS)).isNull()
        bare.accuracy = 4f
        val plain = SampleMapping.location(bare, NOW_ELAPSED_NS, NOW_UNIX_MS)
        assertThat(plain?.tUnixMs).isEqualTo(1_000L)
        assertThat(plain?.hAccM).isEqualTo(4.0)
        assertThat(plain?.altM).isNull()
        assertThat(plain?.speedMps).isNull()
        assertThat(plain?.courseDeg).isNull()
        assertThat(plain?.vAccM).isNull()
        bare.altitude = 34.0
        bare.verticalAccuracyMeters = 6f
        bare.speed = 12f
        bare.speedAccuracyMetersPerSecond = 0.5f
        bare.bearing = 270f
        bare.bearingAccuracyDegrees = 3f
        val full = SampleMapping.location(bare, NOW_ELAPSED_NS, NOW_UNIX_MS)
        assertThat(full?.lat).isEqualTo(52.52)
        assertThat(full?.altM).isEqualTo(34.0)
        assertThat(full?.vAccM).isEqualTo(6.0)
        assertThat(full?.speedMps).isEqualTo(12.0)
        assertThat(full?.speedAccMps).isEqualTo(0.5)
        assertThat(full?.courseDeg).isEqualTo(270.0)
        assertThat(full?.courseAccDeg).isEqualTo(3.0)
    }

    @Test
    fun location_time_follows_the_phone_clock_not_the_gnss_clock() {
        val fix =
            Location("gps").apply {
                latitude = 52.52
                longitude = 13.405
                accuracy = 4f
                time = NOW_UNIX_MS - CLOCK_SKEW_MS
                elapsedRealtimeNanos = NOW_ELAPSED_NS - 300_000_000L
            }
        assertThat(SampleMapping.location(fix, NOW_ELAPSED_NS, NOW_UNIX_MS)?.tUnixMs).isEqualTo(NOW_UNIX_MS - 300L)
    }

    @Test
    fun true_heading_uses_declination() {
        val r = DoubleArray(9)
        val q = DoubleArray(4)
        val yaw = Math.toRadians(-350.0 / 2)
        OrientationMath.quaternion(floatArrayOf(0f, 0f, kotlin.math.sin(yaw).toFloat(), kotlin.math.cos(yaw).toFloat()), q)
        OrientationMath.matrix(q, r)
        val withDeclination = SampleMapping.heading(r, 15.0, 10.0, 5)
        assertThat(withDeclination.magneticDeg).isWithin(0.01).of(350.0)
        assertThat(withDeclination.trueDeg ?: -1.0).isWithin(0.01).of(5.0)
        assertThat(withDeclination.accuracyDeg).isEqualTo(10.0)
        assertThat(SampleMapping.heading(r, null, null, 5).trueDeg).isNull()
    }

    @Test
    fun accuracy_fallback() {
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f, 1f, 0.1f), SensorManager.SENSOR_STATUS_UNRELIABLE) ?: 0.0)
            .isWithin(0.01)
            .of(5.73)
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f, 1f, -1f), SensorManager.SENSOR_STATUS_ACCURACY_HIGH)).isEqualTo(10.0)
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f, 1f, 0f), SensorManager.SENSOR_STATUS_ACCURACY_HIGH)).isEqualTo(10.0)
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f), SensorManager.SENSOR_STATUS_ACCURACY_MEDIUM)).isEqualTo(20.0)
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f), SensorManager.SENSOR_STATUS_ACCURACY_LOW)).isEqualTo(35.0)
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f), SensorManager.SENSOR_STATUS_UNRELIABLE)).isNull()
        assertThat(SampleMapping.accuracyDeg(floatArrayOf(0f, 0f, 0f), SensorManager.SENSOR_STATUS_NO_CONTACT)).isNull()
    }

    @Test
    fun motion_fields() {
        val q = doubleArrayOf(0.9, 0.1, 0.2, 0.3)
        val r = DoubleArray(9)
        OrientationMath.matrix(q, r)
        val heading = SampleMapping.heading(r, 2.0, null, 9)
        val sample = SampleMapping.motion(q, r, floatArrayOf(0.5f, -0.25f, 0.125f), heading, SensorManager.SENSOR_STATUS_ACCURACY_MEDIUM, 9)
        assertThat(sample.frame).isEqualTo(MotionFrame.ENU_MAGNETIC)
        assertThat(listOf(sample.qw, sample.qx, sample.qy, sample.qz)).containsExactly(0.9, 0.1, 0.2, 0.3).inOrder()
        assertThat(listOf(sample.rotX, sample.rotY, sample.rotZ)).containsExactly(0.5, -0.25, 0.125).inOrder()
        assertThat(listOf(sample.gravX, sample.gravY, sample.gravZ)).containsExactly(-r[6], -r[7], -r[8]).inOrder()
        assertThat(sample.headingDeg).isEqualTo(heading.trueDeg)
        assertThat(sample.magAccuracy).isEqualTo(MagAccuracy.MEDIUM)
        assertThat(SampleMapping.magAccuracy(SensorManager.SENSOR_STATUS_UNRELIABLE)).isEqualTo(MagAccuracy.UNCALIBRATED)
        assertThat(SampleMapping.magAccuracy(SensorManager.SENSOR_STATUS_NO_CONTACT)).isEqualTo(MagAccuracy.UNCALIBRATED)
        assertThat(SampleMapping.magAccuracy(SensorManager.SENSOR_STATUS_ACCURACY_LOW)).isEqualTo(MagAccuracy.LOW)
        assertThat(SampleMapping.magAccuracy(SensorManager.SENSOR_STATUS_ACCURACY_HIGH)).isEqualTo(MagAccuracy.HIGH)
    }

    @Test
    fun matrix_and_heading_match_the_platform() {
        val random = Random(7)
        val q = DoubleArray(4)
        val r = DoubleArray(9)
        val platform = FloatArray(9)
        val angles = FloatArray(3)
        repeat(200) {
            val theta = random.nextDouble() * 2 * Math.PI
            val z = random.nextDouble() * 2 - 1
            val phi = random.nextDouble() * 2 * Math.PI
            val radius = sqrt(1 - z * z)
            val half = theta / 2
            val values =
                floatArrayOf(
                    (radius * cos(phi) * sin(half)).toFloat(),
                    (radius * sin(phi) * sin(half)).toFloat(),
                    (z * sin(half)).toFloat(),
                    cos(half).toFloat(),
                )
            OrientationMath.quaternion(values, q)
            OrientationMath.matrix(q, r)
            SensorManager.getRotationMatrixFromVector(platform, values)
            for (i in 0 until 9) assertThat(r[i]).isWithin(1e-5).of(platform[i].toDouble())
            if (hypot(r[1], r[4]) < LEVEL_ENOUGH) return@repeat
            SensorManager.getOrientation(platform, angles)
            val azimuth = OrientationMath.wrap360(Math.toDegrees(angles[0].toDouble()))
            val diff = abs(OrientationMath.magneticHeadingDeg(r) - azimuth)
            assertThat(minOf(diff, 360 - diff)).isWithin(1e-3).of(0.0)
        }
    }

    @Test
    fun unix_millis() {
        val now = 1_700_000_000_000L
        assertThat(SampleMapping.unixMillis(4_000_000_000L, 5_000_000_000L, now)).isEqualTo(now - 1000)
    }

    private companion object {
        const val LEVEL_ENOUGH = 0.05
        const val NOW_UNIX_MS = 1_790_000_000_000L
        const val NOW_ELAPSED_NS = 86_400_000_000_000L
        const val CLOCK_SKEW_MS = 18_000L
    }
}
