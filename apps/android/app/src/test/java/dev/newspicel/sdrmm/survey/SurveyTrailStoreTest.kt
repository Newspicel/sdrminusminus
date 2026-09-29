package dev.newspicel.sdrmm.survey

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.SurveyPoint
import dev.newspicel.sdrmm.ffi.SurveyView
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeServiceControl
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SurveyTrailStoreTest {
    private val core = FakeCoreGateway()
    private val runner = MissionRunner(core, FakeServiceControl())
    private val store = SurveyTrailStore(core, runner) { 0L }

    private fun points(level: Float) = listOf(SurveyPoint(LatLon(52.0, 13.0), level), SurveyPoint(LatLon(52.001, 13.0), level))

    @Test
    fun collects_while_hidden() = runTest {
        store.run(backgroundScope)
        runCurrent()
        runner.open("s1")
        core.survey.value = SurveyView("s1", 433.92e6, -60f, -90f, -40f, 2u, true)
        core.surveyPoints.emit(points(-60f))
        runCurrent()
        assertThat(store.runs.value).isNotEmpty()
        assertThat(store.runs.value.single().coordinates).hasSize(2)
        runner.open("s2")
        runCurrent()
        assertThat(store.runs.value).isEmpty()
    }

    @Test
    fun a_server_clear_empties_the_trail() = runTest {
        store.run(backgroundScope)
        runner.open("s1")
        core.survey.value = SurveyView("s1", 433.92e6, -60f, -90f, -40f, 2u, true)
        runCurrent()
        core.surveyPoints.emit(points(-60f))
        runCurrent()
        assertThat(store.runs.value).isNotEmpty()
        core.survey.value = SurveyView("s1", 433.92e6, -60f, -90f, -40f, 3u, true)
        runCurrent()
        assertThat(store.runs.value).isNotEmpty()
        core.survey.value = SurveyView("s1", 433.92e6, null, -90f, -40f, 0u, true)
        runCurrent()
        assertThat(store.runs.value).isEmpty()
    }

    @Test
    fun reopening_the_same_mission_keeps_the_trail() = runTest {
        store.run(backgroundScope)
        runner.open("s1")
        runCurrent()
        core.surveyPoints.emit(points(-60f))
        runCurrent()
        runner.close()
        runner.open("s1")
        runCurrent()
        assertThat(store.runs.value).isNotEmpty()
        store.clear()
        assertThat(store.runs.value).isEmpty()
    }
}
