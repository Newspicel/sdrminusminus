package dev.newspicel.sdrmm.haptics

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.Trend
import org.junit.Test

class HuntHapticPolicyTest {
    @Test
    fun transitions() {
        val policy = HuntHapticPolicy()
        assertThat(policy.cue(Trend.WAITING, 0.2f, 0)).isNull()
        assertThat(policy.cue(Trend.WARMER, 0.3f, 1_000)).isEqualTo(HapticCue.Increase)
        assertThat(policy.cue(Trend.WARMER, 0.4f, 2_000)).isNull()
        assertThat(policy.cue(Trend.COLDER, 0.3f, 3_000)).isEqualTo(HapticCue.Decrease)
    }

    @Test
    fun success_rearms() {
        val policy = HuntHapticPolicy()
        assertThat(policy.cue(Trend.ON_TOP, 0.95f, 0)).isEqualTo(HapticCue.Success)
        assertThat(policy.cue(Trend.ON_TOP, 0.92f, 1_000)).isNull()
        assertThat(policy.cue(Trend.ON_TOP, 0.7f, 2_000)).isNull()
        assertThat(policy.cue(Trend.ON_TOP, 0.95f, 3_000)).isEqualTo(HapticCue.Success)
    }

    @Test
    fun a_success_held_back_by_the_interval_comes_later() {
        val policy = HuntHapticPolicy()
        assertThat(policy.cue(Trend.WARMER, 0.85f, 0)).isEqualTo(HapticCue.Increase)
        assertThat(policy.cue(Trend.WARMER, 0.95f, 300)).isNull()
        assertThat(policy.cue(Trend.ON_TOP, 0.95f, 800)).isEqualTo(HapticCue.Success)
        assertThat(policy.cue(Trend.ON_TOP, 0.95f, 2_000)).isNull()
    }

    @Test
    fun interval() {
        val policy = HuntHapticPolicy()
        assertThat(policy.cue(Trend.WARMER, 0.3f, 0)).isEqualTo(HapticCue.Increase)
        assertThat(policy.cue(Trend.COLDER, 0.3f, 300)).isNull()
    }
}
