package dev.newspicel.sdrmm.radar

import androidx.annotation.StringRes
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.RadarTrack
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.frames.FrameBitmaps
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.ui.Format
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.util.Locale

data class RadarRow(
    val id: UInt,
    val name: String,
    val range: String,
    val doppler: String,
    @param:StringRes val motion: Int,
    val bearing: String?,
)

data class RadarUiState(
    val title: String,
    val frame: ImageBitmap?,
    val frameVersion: Long,
    val rangeMaxKm: Float,
    val dopplerSpanHz: Float,
    val rows: List<RadarRow>,
    val echoes: UInt,
    val stale: Boolean,
    val problems: List<String>,
    val imageFailed: Boolean,
    val link: LinkState,
)

class RadarViewModel(
    private val missionId: String,
    private val core: CoreGateway,
    private val router: NoticeRouter,
    private val decode: CoroutineDispatcher = Dispatchers.Default,
) : ViewModel() {
    private val bitmaps = FrameBitmaps()
    private val current =
        MutableStateFlow(
            RadarUiState(
                title = title(),
                frame = null,
                frameVersion = 0,
                rangeMaxKm = 0f,
                dopplerSpanHz = 0f,
                rows = emptyList(),
                echoes = 0u,
                stale = false,
                problems = emptyList(),
                imageFailed = false,
                link = core.link.value,
            ),
        )
    val state: StateFlow<RadarUiState> = current.asStateFlow()

    init {
        viewModelScope.launch {
            combine(core.radar, core.link, core.missions) { radar, link, _ -> radar?.takeIf { it.mission == missionId } to link }
                .collect { (radar, link) -> current.update { apply(it, radar, link) } }
        }
        viewModelScope.launch(decode) { core.radarImage.filterNotNull().collect(::decode) }
    }

    private fun apply(
        state: RadarUiState,
        radar: RadarView?,
        link: LinkState,
    ): RadarUiState = state.copy(
        title = title(),
        rows = rows(radar?.tracks.orEmpty()),
        echoes = radar?.echoes ?: 0u,
        stale = radar?.stale == true,
        problems = radar?.problems.orEmpty(),
        link = link,
    )

    private fun decode(image: RgbaImage) {
        when (val decoded = bitmaps.update(image)) {
            is Outcome.Ok -> {
                val frame = decoded.value.asImageBitmap()
                current.update {
                    it.copy(
                        frame = frame,
                        frameVersion = it.frameVersion + 1,
                        rangeMaxKm = image.rangeMaxKm,
                        dopplerSpanHz = image.dopplerSpanHz,
                        imageFailed = false,
                    )
                }
            }

            is Outcome.Failed -> {
                if (!current.value.imageFailed) router.report(decoded.error)
                current.update { it.copy(imageFailed = true) }
            }
        }
    }

    private fun title(): String = core.missions.value
        ?.missions
        ?.firstOrNull { it.id == missionId }
        ?.title ?: missionId

    companion object {
        const val MAX_ROWS = 8

        fun rows(tracks: List<RadarTrack>): List<RadarRow> = tracks.sortedByDescending { it.snrDb }.take(MAX_ROWS).map { track ->
            RadarRow(
                id = track.id,
                name = String.format(Locale.ROOT, "T%02d", track.id.toLong()),
                range = String.format(Locale.ROOT, "%.1f km", track.rangeKm),
                doppler = String.format(Locale.ROOT, "%+.0f Hz", track.dopplerHz),
                motion = if (track.closing) R.string.radar_closing else R.string.radar_opening,
                bearing = track.bearingDeg?.let { Format.angle(it.toDouble()) },
            )
        }
    }
}
