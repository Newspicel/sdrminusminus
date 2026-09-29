package dev.newspicel.sdrmm.survey

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.SurveyPoint
import dev.newspicel.sdrmm.ffi.SurveyView
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(AndroidJUnit4::class)
class SurveyViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway()
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core) }

    @Test
    fun trail_from_points() = runTest {
        core.missions.value = Samples.missions(Samples.mission("s1", MissionKind.SURVEY, "Walk").copy(controls = listOf(MissionControl.SURVEY_RUN)))
        graph.surveyTrail.run(backgroundScope)
        runCurrent()
        val model = SurveyViewModel("s1", core, graph.sensors, graph.runner, graph.surveyTrail, graph.router)
        core.survey.value = SurveyView("s1", 433.92e6, -60f, -90f, -40f, 2u, false)
        core.surveyPoints.emit(listOf(SurveyPoint(LatLon(52.0, 13.0), -60f), SurveyPoint(LatLon(52.001, 13.0), -41f)))
        runCurrent()
        assertThat(model.state.value.runs).isNotEmpty()
        assertThat(model.extent()).contains(LatLon(52.001, 13.0))
        model.clearTrail()
        runCurrent()
        assertThat(model.state.value.runs).isEmpty()
    }

    @Test
    fun record_toggles_the_server_survey() {
        core.missions.value = Samples.missions(Samples.mission("s1", MissionKind.SURVEY, "Walk").copy(controls = listOf(MissionControl.SURVEY_RUN)))
        val model = SurveyViewModel("s1", core, graph.sensors, graph.runner, graph.surveyTrail, graph.router)
        core.survey.value = SurveyView("s1", 433.92e6, -60f, -90f, -40f, 2u, false)
        model.toggleRecord()
        core.survey.value = SurveyView("s1", 433.92e6, -60f, -90f, -40f, 2u, true)
        model.toggleRecord()
        assertThat(core.commands).containsExactly(MissionCommand.StartSurvey, MissionCommand.StopSurvey).inOrder()
        model.fit()
        assertThat(model.state.value.fitRequest).isEqualTo(1)
    }
}
