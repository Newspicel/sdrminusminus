package dev.newspicel.sdrmm.nav

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import androidx.test.core.app.ApplicationProvider
import androidx.test.espresso.intent.Intents
import androidx.test.espresso.intent.Intents.intended
import androidx.test.espresso.intent.Intents.intending
import androidx.test.espresso.intent.matcher.IntentMatchers.anyIntent
import androidx.test.espresso.intent.matcher.IntentMatchers.hasAction
import androidx.test.espresso.intent.matcher.IntentMatchers.hasData
import androidx.test.espresso.intent.matcher.IntentMatchers.hasExtra
import androidx.test.ext.junit.rules.ActivityScenarioRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.newspicel.sdrmm.MainActivity
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.settings.NavChoice
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import org.hamcrest.Matchers.allOf
import org.junit.After
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NavHandoffDeviceTest {
    @get:Rule
    val scenario = ActivityScenarioRule(MainActivity::class.java)

    @Before
    fun record() {
        Intents.init()
        intending(anyIntent()).respondWith(Instrumentation.ActivityResult(Activity.RESULT_OK, null))
    }

    @After
    fun release() {
        Intents.release()
    }

    @Test
    fun ask_opens_a_chooser_with_geo_data() {
        val settings = FakeSettingsStore(AppSettings(phoneName = "Pixel", navChoice = NavChoice.Ask))
        val handoff = NavHandoff(ApplicationProvider.getApplicationContext(), FakeCoreGateway(), settings)
        val uri = "geo:52.520000,13.405000"
        assumeTrue(handoff.intentFor(LatLon(52.52, 13.405)).resolves)
        scenario.scenario.onActivity { activity -> handoff.open(activity, "d1", LatLon(52.52, 13.405)) }
        intended(allOf(hasAction(Intent.ACTION_CHOOSER), hasExtra(org.hamcrest.Matchers.equalTo(Intent.EXTRA_INTENT), hasData(uri))))
    }
}
