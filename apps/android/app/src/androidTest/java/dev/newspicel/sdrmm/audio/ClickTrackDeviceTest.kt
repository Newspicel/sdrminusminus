package dev.newspicel.sdrmm.audio

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ClickTrackDeviceTest {
    @Test
    fun plays_and_stops() {
        val track = AudioClickTrack(ApplicationProvider.getApplicationContext())
        track.setStrength(0.8f)
        track.start()
        assertThat(track.state.value).isEqualTo(ClickState.Playing)
        Thread.sleep(PLAY_MS)
        assertThat(track.state.value).isEqualTo(ClickState.Playing)
        track.stop()
        assertThat(track.state.value).isEqualTo(ClickState.Stopped)
    }

    private companion object {
        const val PLAY_MS = 1_000L
    }
}
