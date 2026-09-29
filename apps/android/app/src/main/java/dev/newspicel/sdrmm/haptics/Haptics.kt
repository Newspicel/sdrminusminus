package dev.newspicel.sdrmm.haptics

import android.content.Context
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager

interface Haptics {
    val available: Boolean

    fun play(cue: HapticCue)
}

class VibratorHaptics(
    context: Context,
) : Haptics {
    private val vibrator: Vibrator? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            context.getSystemService(Vibrator::class.java)
        }

    override val available: Boolean = vibrator?.hasVibrator() == true

    override fun play(cue: HapticCue) {
        val effect =
            when (cue) {
                HapticCue.Increase -> VibrationEffect.EFFECT_CLICK
                HapticCue.Decrease -> VibrationEffect.EFFECT_DOUBLE_CLICK
                HapticCue.Success -> VibrationEffect.EFFECT_HEAVY_CLICK
            }
        if (available) vibrator?.vibrate(VibrationEffect.createPredefined(effect))
    }
}
