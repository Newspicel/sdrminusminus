package dev.newspicel.sdrmm.speech

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Bundle
import android.os.SystemClock
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import android.speech.tts.Voice
import dev.newspicel.sdrmm.settings.SettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

sealed interface SpeakerState {
    data object Starting : SpeakerState

    data object Ready : SpeakerState

    data class Failed(
        val reason: String,
    ) : SpeakerState
}

interface Speaker {
    val state: StateFlow<SpeakerState>
    val voices: StateFlow<List<String>>

    fun say(
        text: String,
        urgent: Boolean,
    )

    fun shutdown()
}

class TtsSpeaker(
    context: Context,
    private val settings: SettingsStore,
    private val scope: CoroutineScope,
) : Speaker {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val current = MutableStateFlow<SpeakerState>(SpeakerState.Starting)
    private val installed = MutableStateFlow<List<String>>(emptyList())
    private val pending = ConcurrentHashMap<String, Long>()
    private val ids = AtomicLong()
    private val attributes =
        AudioAttributes
            .Builder()
            .setUsage(AudioAttributes.USAGE_ASSISTANCE_NAVIGATION_GUIDANCE)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
    private val focus =
        AudioFocusRequest
            .Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
            .setAudioAttributes(attributes)
            .build()
    private var follow: Job? = null
    private val progress =
        object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String) = Unit

            override fun onDone(utteranceId: String) = finished(utteranceId)

            override fun onStop(
                utteranceId: String,
                interrupted: Boolean,
            ) = finished(utteranceId)

            override fun onError(
                utteranceId: String,
                errorCode: Int,
            ) {
                current.value = SpeakerState.Failed("Speech error $errorCode")
                finished(utteranceId)
            }

            @Deprecated("Replaced by onError(String, Int)")
            override fun onError(utteranceId: String) {
                onError(utteranceId, TextToSpeech.ERROR)
            }
        }

    private val tts: TextToSpeech = TextToSpeech(context.applicationContext, ::initialized)

    override val state: StateFlow<SpeakerState> = current.asStateFlow()
    override val voices: StateFlow<List<String>> = installed.asStateFlow()

    private fun initialized(status: Int) {
        if (status != TextToSpeech.SUCCESS) {
            current.value = SpeakerState.Failed("Speech engine failed ($status)")
            return
        }
        tts.setAudioAttributes(attributes)
        tts.setOnUtteranceProgressListener(progress)
        val language = pickLanguage()
        if (language == null) {
            current.value = SpeakerState.Failed("No voice for ${Locale.getDefault().toLanguageTag()}")
            return
        }
        installed.value = voicesFor(language).map { it.name }
        current.value = SpeakerState.Ready
        follow =
            scope.launch {
                settings.settings.map { it.voiceName }.distinctUntilChanged().collect(::useVoice)
            }
    }

    private fun pickLanguage(): Locale? = listOf(Locale.getDefault(), Locale.ENGLISH).firstOrNull { locale ->
        when (tts.setLanguage(locale)) {
            TextToSpeech.LANG_MISSING_DATA, TextToSpeech.LANG_NOT_SUPPORTED -> false
            else -> true
        }
    }

    private fun voicesFor(language: Locale): List<Voice> = tts.voices
        .orEmpty()
        .filter { it.locale.language == language.language && !it.isNetworkConnectionRequired }
        .sortedByDescending { it.quality }

    private fun useVoice(name: String?) {
        val voice = name?.let { wanted -> tts.voices.orEmpty().firstOrNull { it.name == wanted } } ?: return
        if (tts.setVoice(voice) == TextToSpeech.ERROR) current.value = SpeakerState.Failed("Voice $name failed")
    }

    override fun say(
        text: String,
        urgent: Boolean,
    ) {
        if (current.value != SpeakerState.Ready) return
        val now = SystemClock.elapsedRealtime()
        val stale = pending.values.any { now - it > STALE_MS }
        val mode = if (urgent || stale) TextToSpeech.QUEUE_FLUSH else TextToSpeech.QUEUE_ADD
        if (mode == TextToSpeech.QUEUE_FLUSH) pending.clear()
        if (pending.isEmpty()) audio?.requestAudioFocus(focus)
        val id = "u${ids.incrementAndGet()}"
        pending[id] = now
        if (tts.speak(text, mode, Bundle(), id) == TextToSpeech.ERROR) {
            pending.remove(id)
            current.value = SpeakerState.Failed("Speech failed")
            release()
        }
    }

    private fun finished(id: String) {
        pending.remove(id)
        if (pending.isEmpty()) release()
    }

    private fun release() {
        audio?.abandonAudioFocusRequest(focus)
    }

    override fun shutdown() {
        follow?.cancel()
        tts.stop()
        tts.shutdown()
        release()
    }

    private companion object {
        const val STALE_MS = 5_000L
    }
}
