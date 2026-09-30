package dev.newspicel.sdrmm.mission

import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.ErrorText
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.nav.HandoffResult
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.speech.SpeechText
import dev.newspicel.sdrmm.ui.Format
import dev.newspicel.sdrmm.ui.components.UiText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicLong

sealed interface Banner {
    val id: Long
    val level: NoticeLevel

    data class Text(
        override val id: Long,
        override val level: NoticeLevel,
        val text: UiText,
        val detail: String?,
    ) : Banner

    data class Retarget(
        override val id: Long,
        val notice: RetargetNotice,
        val distanceText: String?,
    ) : Banner {
        override val level: NoticeLevel = NoticeLevel.INFO
    }
}

class Retargets(
    val lastFix: StateFlow<LocationSample?>,
    val handedOff: StateFlow<Set<String>>,
    val appResumed: StateFlow<Boolean>,
    val speak: (String) -> Unit,
    val notify: (RetargetNotice, String) -> Unit,
)

class NoticeRouter(
    private val core: CoreGateway,
    private val settings: SettingsStore,
    private val retargets: Retargets,
) {
    private val ids = AtomicLong()
    private val queue = MutableStateFlow<List<Banner>>(emptyList())
    val banners: StateFlow<List<Banner>> = queue.asStateFlow()

    fun run(scope: CoroutineScope): Job = scope.launch(start = CoroutineStart.UNDISPATCHED) {
        launch(start = CoroutineStart.UNDISPATCHED) {
            core.notices.collect { show(it.level, UiText.Raw(it.text)) }
        }
        launch(start = CoroutineStart.UNDISPATCHED) {
            core.retargets.collect(::retarget)
        }
        launch(start = CoroutineStart.UNDISPATCHED) {
            var seen = core.missedUpdates.value
            core.missedUpdates.collect { missed ->
                if (missed > seen) show(NoticeLevel.WARN, UiText.Plural(R.plurals.missed_updates, missed.toInt()))
                seen = missed
            }
        }
        launch(start = CoroutineStart.UNDISPATCHED) {
            settings.readFailed.collect { failed -> if (failed) show(NoticeLevel.WARN, UiText.Res(R.string.settings_reset)) }
        }
        launch(start = CoroutineStart.UNDISPATCHED) {
            settings.writeFailed.collect { reason -> if (reason != null) show(NoticeLevel.ERROR, UiText.Res(R.string.settings_not_saved), reason) }
        }
    }

    fun retarget(notice: RetargetNotice) {
        val fix = retargets.lastFix.value?.let { LatLon(it.lat, it.lon) }
        val distanceM = fix?.let { core.distanceM(it, notice.target.at) }
        val units = Format.resolve(settings.settings.value.units)
        val distanceText = distanceM?.let { Format.distance(it, units) }
        push(Banner.Retarget(ids.incrementAndGet(), notice, distanceText))
        if (notice.reason == RetargetReason.FIRST) return
        if (settings.settings.value.voice) {
            val bearing = fix?.let { core.bearingDeg(it, notice.target.at) }
            retargets.speak(SpeechText.retarget(notice, distanceM, bearing, units))
        }
        val handedOff = notice.mission in retargets.handedOff.value
        if (handedOff && !retargets.appResumed.value) retargets.notify(notice, distanceText ?: "-")
    }

    fun handoff(result: HandoffResult) {
        when (result) {
            HandoffResult.Opened -> Unit
            HandoffResult.FellBack -> show(NoticeLevel.INFO, UiText.Res(R.string.google_maps_missing))
            is HandoffResult.Failed -> show(NoticeLevel.ERROR, UiText.Res(result.reason))
        }
    }

    fun report(error: CoreException) {
        show(NoticeLevel.ERROR, ErrorText.short(error), ErrorText.detail(error))
    }

    fun show(
        level: NoticeLevel,
        text: UiText,
        detail: String? = null,
    ) {
        push(Banner.Text(ids.incrementAndGet(), level, text, detail))
    }

    private fun push(banner: Banner) {
        queue.update { (it + banner).takeLast(MAX_QUEUED) }
    }

    fun dismiss(banner: Banner) {
        queue.update { list -> list.filterNot { it.id == banner.id } }
    }

    private companion object {
        const val MAX_QUEUED = 20
    }
}
