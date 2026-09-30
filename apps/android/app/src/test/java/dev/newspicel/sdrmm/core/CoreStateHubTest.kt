package dev.newspicel.sdrmm.core

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.demo.DemoScript
import dev.newspicel.sdrmm.ffi.CoreEvent
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.NavPoint
import dev.newspicel.sdrmm.ffi.Notice
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.ffi.SurveyPoint
import dev.newspicel.sdrmm.ffi.SurveyView
import dev.newspicel.sdrmm.testing.Samples
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class CoreStateHubTest {
    @Test
    fun routes_events() = runTest {
        val hub = CoreStateHub()
        val script = DemoScript()
        val notices = mutableListOf<Notice>()
        val retargets = mutableListOf<RetargetNotice>()
        val points = mutableListOf<List<SurveyPoint>>()
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { hub.notices.collect { notices += it } }
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { hub.retargets.collect { retargets += it } }
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { hub.surveyPoints.collect { points += it } }
        val missions = Samples.missions(Samples.mission())
        val pose = Samples.pose()
        val hunt = script.hunt(1, true)
        val df = script.df(1)
        val radar = RadarView("r", 2u, emptyList(), false, emptyList())
        val image = RgbaImage(1u, 1u, byteArrayOf(1, 2, 3, 4), 10f, 20f)
        val survey = SurveyView("s", 433e6, -80f, -100f, -40f, 3u, true)
        val point = SurveyPoint(LatLon(1.0, 2.0), -70f)
        val retarget = RetargetNotice("d", NavPoint(LatLon(1.0, 2.0), GuidanceKind.PROBE), 300.0, RetargetReason.MOVED)
        val notice = Notice(NoticeLevel.WARN, "Missed 2 updates")
        listOf(
            CoreEvent.Link(LinkState.Online("Bench")),
            CoreEvent.Missions(missions),
            CoreEvent.Pose(pose),
            CoreEvent.Hunt(hunt),
            CoreEvent.Df(df),
            CoreEvent.Radar(radar),
            CoreEvent.RadarImage(image),
            CoreEvent.Survey(survey),
            CoreEvent.SurveyPoints(listOf(point)),
            CoreEvent.Retarget(retarget),
            CoreEvent.Notice(notice),
        ).forEach(hub::dispatch)
        assertThat(hub.link.value).isEqualTo(LinkState.Online("Bench"))
        assertThat(hub.missions.value).isEqualTo(missions)
        assertThat(hub.pose.value).isEqualTo(pose)
        assertThat(hub.hunt.value).isEqualTo(hunt)
        assertThat(hub.df.value).isEqualTo(df)
        assertThat(hub.radar.value).isEqualTo(radar)
        assertThat(hub.radarImage.value?.rgba).isEqualTo(byteArrayOf(1, 2, 3, 4))
        assertThat(hub.survey.value).isEqualTo(survey)
        assertThat(points).containsExactly(listOf(point))
        assertThat(retargets).containsExactly(retarget)
        assertThat(notices).containsExactly(notice)
        assertThat(hub.missedUpdates.value).isEqualTo(0L)
    }

    @Test
    fun overflow_counts() = runTest {
        val hub = CoreStateHub()
        backgroundScope.launch { hub.notices.collect { awaitCancellation() } }
        runCurrent()
        repeat(CoreStateHub.BUFFER + 1) { hub.dispatch(CoreEvent.Notice(Notice(NoticeLevel.INFO, "n$it"))) }
        assertThat(hub.missedUpdates.value).isEqualTo(1L)
    }
}
