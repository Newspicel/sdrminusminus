package dev.newspicel.sdrmm.sensors

import com.google.common.truth.Truth.assertThat
import org.junit.Test

class OrientationMathTest {
    private val q = DoubleArray(4)
    private val r = DoubleArray(9)
    private val g = DoubleArray(3)

    private fun orient(vararg values: Float) {
        OrientationMath.quaternion(values, q)
        OrientationMath.matrix(q, r)
        OrientationMath.gravityG(r, g)
    }

    @Test
    fun identity_heads_north() {
        orient(0f, 0f, 0f, 1f)
        assertThat(OrientationMath.magneticHeadingDeg(r)).isWithin(1e-9).of(0.0)
        assertThat(g[0]).isWithin(1e-9).of(0.0)
        assertThat(g[1]).isWithin(1e-9).of(0.0)
        assertThat(g[2]).isWithin(1e-9).of(-1.0)
    }

    @Test
    fun yaw_minus_90_heads_east() {
        orient(0f, 0f, -0.7071f, 0.7071f)
        assertThat(OrientationMath.magneticHeadingDeg(r)).isWithin(0.01).of(90.0)
    }

    @Test
    fun missing_w_reconstructed() {
        orient(0.1f, -0.2f, 0.3f)
        val three = r.copyOf()
        val w = kotlin.math.sqrt(1.0 - 0.01 - 0.04 - 0.09).toFloat()
        orient(0.1f, -0.2f, 0.3f, w)
        for (i in 0 until 9) assertThat(three[i]).isWithin(1e-6).of(r[i])
    }

    @Test
    fun upright_gravity() {
        orient(0.7071f, 0f, 0f, 0.7071f)
        assertThat(g[0]).isWithin(1e-6).of(0.0)
        assertThat(g[1]).isWithin(1e-4).of(-1.0)
        assertThat(g[2]).isWithin(1e-4).of(0.0)
    }

    @Test
    fun wrap() {
        assertThat(OrientationMath.wrap360(-10.0)).isEqualTo(350.0)
        assertThat(OrientationMath.wrap360(725.0)).isEqualTo(5.0)
        assertThat(OrientationMath.wrap360(360.0)).isEqualTo(0.0)
    }
}
