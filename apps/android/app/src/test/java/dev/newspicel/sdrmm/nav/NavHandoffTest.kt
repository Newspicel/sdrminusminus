package dev.newspicel.sdrmm.nav

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.IntentFilter
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.settings.NavChoice
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.Shadows.shadowOf

@RunWith(AndroidJUnit4::class)
class NavHandoffTest {
    private val settings = FakeSettingsStore(AppSettings(phoneName = "Pixel", navChoice = NavChoice.GoogleMaps))
    private val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
    private val handoff = NavHandoff(ApplicationProvider.getApplicationContext(), FakeCoreGateway(), settings)
    private val target = LatLon(52.52, 13.405)

    private fun install(
        packageName: String,
        scheme: String,
    ) {
        val component = ComponentName(packageName, "$packageName.Main")
        val packages = shadowOf(ApplicationProvider.getApplicationContext<android.app.Application>().packageManager)
        packages.addActivityIfNotPresent(component)
        packages.addIntentFilterForActivity(
            component,
            IntentFilter(Intent.ACTION_VIEW).apply {
                addCategory(Intent.CATEGORY_DEFAULT)
                addDataScheme(scheme)
            },
        )
    }

    @Test
    fun google_maps_when_installed() {
        install(NavIntents.MAPS_PACKAGE, "geo")
        install(NavIntents.MAPS_PACKAGE, "google.navigation")
        assertThat(handoff.open(activity, "d1", target)).isEqualTo(HandoffResult.Opened)
        val started = shadowOf(activity).nextStartedActivity
        assertThat(started.`package`).isEqualTo(NavIntents.MAPS_PACKAGE)
        assertThat(handoff.handedOff.value).containsExactly("d1")
    }

    @Test
    fun fallback() {
        install("org.example.maps", "geo")
        assertThat(handoff.open(activity, "d1", target)).isEqualTo(HandoffResult.FellBack)
        val started = shadowOf(activity).nextStartedActivity
        assertThat(started.action).isEqualTo(Intent.ACTION_CHOOSER)
        assertThat(handoff.handedOff.value).containsExactly("d1")
    }

    @Test
    fun no_app() {
        assertThat(handoff.open(activity, "d1", target)).isEqualTo(HandoffResult.Failed(R.string.no_map_app))
        assertThat(shadowOf(activity).nextStartedActivity).isNull()
        assertThat(handoff.handedOff.value).isEmpty()
    }

    @Test
    fun ask_uses_the_chooser() {
        install(NavIntents.MAPS_PACKAGE, "geo")
        settings.settings.value = settings.settings.value.copy(navChoice = NavChoice.Ask)
        val handed = handoff.intentFor(target)
        assertThat(handed.intent.action).isEqualTo(Intent.ACTION_CHOOSER)
        assertThat(handed.fellBack).isFalse()
    }
}
