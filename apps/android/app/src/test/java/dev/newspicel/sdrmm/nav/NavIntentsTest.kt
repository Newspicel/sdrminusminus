package dev.newspicel.sdrmm.nav

import android.content.Intent
import androidx.car.app.CarContext
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NavIntentsTest {
    @Test
    fun google_maps() {
        val intent = NavIntents.googleMaps("google.navigation:q=52.520000,13.405000&mode=d")
        assertThat(intent.`package`).isEqualTo("com.google.android.apps.maps")
        assertThat(intent.action).isEqualTo(Intent.ACTION_VIEW)
        assertThat(intent.dataString).isEqualTo("google.navigation:q=52.520000,13.405000&mode=d")
    }

    @Test
    fun chooser_wraps_the_geo_intent() {
        val chooser = NavIntents.chooser("geo:1.000000,2.000000?q=1.000000,2.000000(Target)", "Navigate")
        assertThat(chooser.action).isEqualTo(Intent.ACTION_CHOOSER)
        val inner = chooser.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
        assertThat(inner?.dataString).isEqualTo("geo:1.000000,2.000000?q=1.000000,2.000000(Target)")
        assertThat(chooser.getCharSequenceExtra(Intent.EXTRA_TITLE)).isEqualTo("Navigate")
    }

    @Test
    fun car_uses_navigate_action() {
        val intent = NavIntents.car("geo:1.000000,2.000000")
        assertThat(intent.action).isEqualTo(CarContext.ACTION_NAVIGATE)
        assertThat(intent.dataString).isEqualTo("geo:1.000000,2.000000")
    }
}
