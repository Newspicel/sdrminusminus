package dev.newspicel.sdrmm.core

import dev.newspicel.sdrmm.ffi.CoreEvent
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.HuntView
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionsView
import dev.newspicel.sdrmm.ffi.Notice
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.ffi.SurveyPoint
import dev.newspicel.sdrmm.ffi.SurveyView
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update

class CoreStateHub {
    val link = MutableStateFlow<LinkState>(LinkState.Offline)
    val missions = MutableStateFlow<MissionsView?>(null)
    val pose = MutableStateFlow<PoseView?>(null)
    val hunt = MutableStateFlow<HuntView?>(null)
    val df = MutableStateFlow<DfView?>(null)
    val radar = MutableStateFlow<RadarView?>(null)
    val radarImage = MutableStateFlow<RgbaImage?>(null)
    val survey = MutableStateFlow<SurveyView?>(null)
    val surveyPoints = MutableSharedFlow<List<SurveyPoint>>(extraBufferCapacity = BUFFER)
    val retargets = MutableSharedFlow<RetargetNotice>(extraBufferCapacity = BUFFER)
    val notices = MutableSharedFlow<Notice>(extraBufferCapacity = BUFFER)
    val missedUpdates = MutableStateFlow(0L)

    fun dispatch(event: CoreEvent) {
        when (event) {
            is CoreEvent.Link -> link.value = event.state
            is CoreEvent.Missions -> missions.value = event.view
            is CoreEvent.Pose -> pose.value = event.view
            is CoreEvent.Hunt -> hunt.value = event.view
            is CoreEvent.Df -> df.value = event.view
            is CoreEvent.Radar -> radar.value = event.view
            is CoreEvent.RadarImage -> radarImage.value = event.image
            is CoreEvent.Survey -> survey.value = event.view
            is CoreEvent.SurveyPoints -> offer(surveyPoints, event.points)
            is CoreEvent.Retarget -> offer(retargets, event.notice)
            is CoreEvent.Notice -> offer(notices, event.notice)
        }
    }

    private fun <T> offer(
        flow: MutableSharedFlow<T>,
        value: T,
    ) {
        if (!flow.tryEmit(value)) missedUpdates.update { it + 1 }
    }

    companion object {
        const val BUFFER = 256
    }
}
