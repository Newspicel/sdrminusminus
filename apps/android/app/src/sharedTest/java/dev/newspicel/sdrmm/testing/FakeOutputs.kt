package dev.newspicel.sdrmm.testing

import android.app.Application
import dev.newspicel.sdrmm.OutputFactory
import dev.newspicel.sdrmm.Outputs
import dev.newspicel.sdrmm.audio.ClickState
import dev.newspicel.sdrmm.audio.ClickTrack
import dev.newspicel.sdrmm.haptics.HapticCue
import dev.newspicel.sdrmm.haptics.Haptics
import dev.newspicel.sdrmm.mission.ServiceControl
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.speech.Speaker
import dev.newspicel.sdrmm.speech.SpeakerState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import java.util.Collections

class FakeSpeaker : Speaker {
    override val state = MutableStateFlow<SpeakerState>(SpeakerState.Ready)
    override val voices = MutableStateFlow(listOf("en-us-x-test"))
    val said: MutableList<Pair<String, Boolean>> = Collections.synchronizedList(mutableListOf())

    override fun say(
        text: String,
        urgent: Boolean,
    ) {
        said += text to urgent
    }

    override fun shutdown() = Unit
}

class FakeClickTrack : ClickTrack {
    override val state = MutableStateFlow<ClickState>(ClickState.Stopped)
    val strengths: MutableList<Float> = Collections.synchronizedList(mutableListOf())

    override fun start() {
        if (state.value !is ClickState.Failed) state.value = ClickState.Playing
    }

    override fun stop() {
        if (state.value == ClickState.Playing) state.value = ClickState.Stopped
    }

    override fun setStrength(strength: Float) {
        strengths += strength
    }
}

class FakeHaptics(
    override val available: Boolean = true,
) : Haptics {
    val played: MutableList<HapticCue> = Collections.synchronizedList(mutableListOf())

    override fun play(cue: HapticCue) {
        played += cue
    }
}

class FakeServiceControl(
    var refusal: String? = null,
) : ServiceControl {
    var starts = 0
    var stops = 0

    override fun start(): String? {
        starts += 1
        return refusal
    }

    override fun stop() {
        stops += 1
    }
}

class FakeOutputs(
    val speaker: FakeSpeaker = FakeSpeaker(),
    val clickTrack: FakeClickTrack = FakeClickTrack(),
    val haptics: FakeHaptics = FakeHaptics(),
    val service: ServiceControl = FakeServiceControl(),
) : OutputFactory {
    override fun create(
        app: Application,
        settings: SettingsStore,
        scope: CoroutineScope,
    ): Outputs = Outputs(speaker, clickTrack, haptics, service)
}
