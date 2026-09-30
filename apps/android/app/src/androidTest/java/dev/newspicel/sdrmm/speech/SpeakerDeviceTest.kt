package dev.newspicel.sdrmm.speech

import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SpeakerDeviceTest {
    @Test
    fun leaves_starting() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        lateinit var speaker: TtsSpeaker
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            speaker = TtsSpeaker(ApplicationProvider.getApplicationContext(), FakeSettingsStore(), scope)
        }
        val deadline = SystemClock.elapsedRealtime() + PATIENCE_MS
        while (speaker.state.value == SpeakerState.Starting && SystemClock.elapsedRealtime() < deadline) Thread.sleep(STEP_MS)
        assertThat(speaker.state.value).isNotEqualTo(SpeakerState.Starting)
        speaker.say("Voice test", urgent = true)
        speaker.shutdown()
        scope.cancel()
    }

    private companion object {
        const val PATIENCE_MS = 5_000L
        const val STEP_MS = 100L
    }
}
