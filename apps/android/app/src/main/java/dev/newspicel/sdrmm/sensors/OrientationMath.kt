package dev.newspicel.sdrmm.sensors

import kotlin.math.atan2
import kotlin.math.max
import kotlin.math.sqrt

object OrientationMath {
    fun quaternion(
        values: FloatArray,
        out: DoubleArray,
    ) {
        val x = values[0].toDouble()
        val y = values[1].toDouble()
        val z = values[2].toDouble()
        out[0] = if (values.size > 3) values[3].toDouble() else sqrt(max(0.0, 1.0 - x * x - y * y - z * z))
        out[1] = x
        out[2] = y
        out[3] = z
    }

    fun matrix(
        q: DoubleArray,
        out: DoubleArray,
    ) {
        val w = q[0]
        val x = q[1]
        val y = q[2]
        val z = q[3]
        out[0] = 1 - 2 * (y * y + z * z)
        out[1] = 2 * (x * y - z * w)
        out[2] = 2 * (x * z + y * w)
        out[3] = 2 * (x * y + z * w)
        out[4] = 1 - 2 * (x * x + z * z)
        out[5] = 2 * (y * z - x * w)
        out[6] = 2 * (x * z - y * w)
        out[7] = 2 * (y * z + x * w)
        out[8] = 1 - 2 * (x * x + y * y)
    }

    fun magneticHeadingDeg(r: DoubleArray): Double = wrap360(Math.toDegrees(atan2(r[1], r[4])))

    fun gravityG(
        r: DoubleArray,
        out: DoubleArray,
    ) {
        out[0] = -r[6]
        out[1] = -r[7]
        out[2] = -r[8]
    }

    fun wrap360(deg: Double): Double {
        val wrapped = deg % 360.0
        return if (wrapped < 0) wrapped + 360.0 else wrapped
    }
}
