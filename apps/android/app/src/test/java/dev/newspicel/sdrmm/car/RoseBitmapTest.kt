package dev.newspicel.sdrmm.car

import androidx.compose.ui.graphics.toArgb
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.df.RoseGeometry
import dev.newspicel.sdrmm.df.RoseModel
import dev.newspicel.sdrmm.ui.theme.DarkColors
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.annotation.GraphicsMode

@RunWith(AndroidJUnit4::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class RoseBitmapTest {
    @Test
    fun draws() {
        val model = RoseModel(bearingDeg = 90.0, sigmaDeg = null, guidanceDeg = null, northDeg = 0.0, headingUp = false)
        val bitmap = RoseBitmap.draw(model, 256, dark = true)
        assertThat(bitmap.width).isEqualTo(256)
        assertThat(bitmap.height).isEqualTo(256)
        val needle = RoseGeometry.point(90.0, 128f * 0.6f, 128f, 128f)
        assertThat(bitmap.getPixel(needle.x.toInt(), needle.y.toInt())).isEqualTo(DarkColors.primary.toArgb())
    }
}
