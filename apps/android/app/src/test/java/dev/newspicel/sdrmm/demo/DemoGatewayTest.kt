package dev.newspicel.sdrmm.demo

import app.cash.turbine.test
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.ffi.SweepPhase
import kotlinx.coroutines.test.runTest
import org.junit.Test

class DemoGatewayTest {
    private val demo = DemoGateway(base = null)

    @Test
    fun lists_hunt_df_and_radar_missions_online() {
        assertThat(demo.link.value).isEqualTo(LinkState.Online(DemoGateway.SERVER_NAME))
        assertThat(
            demo.missions.value
                ?.missions
                ?.map { it.kind },
        ).containsExactly(MissionKind.HUNT, MissionKind.DF_DRIVE, MissionKind.RADAR_WATCH, MissionKind.SURVEY)
            .inOrder()
        assertThat((demo.savedServers() as Outcome.Ok).value.single().id).isEqualTo(DemoGateway.SERVER_ID)
    }

    @Test
    fun an_open_mission_is_scripted() = runTest {
        assertThat(demo.openMission(DemoScript.HUNT_ID)).isEqualTo(Outcome.Ok(Unit))
        demo.step(3)
        assertThat(demo.hunt.value?.readings).isEqualTo(3uL)
        demo.send(MissionCommand.StopHunt)
        assertThat(demo.hunt.value?.running).isFalse()
        demo.send(MissionCommand.Tune(145.5e6))
        demo.step(4)
        assertThat(demo.hunt.value?.freqHz).isEqualTo(145.5e6)
        demo.openMission(DemoScript.DF_ID)
        demo.step(5)
        assertThat(demo.df.value?.bearingTrueDeg).isEqualTo(15f)
        assertThat(demo.df.value?.target).isNotNull()
        demo.openMission(DemoScript.RADAR_ID)
        demo.step(6)
        assertThat(demo.radar.value?.tracks).hasSize(3)
        val image = demo.radarImage.value
        assertThat(image?.rgba?.size).isEqualTo((image?.width ?: 0u).toInt() * (image?.height ?: 0u).toInt() * 4)
    }

    @Test
    fun a_sweep_turns_the_phone_and_finds_the_source() = runTest {
        demo.openMission(DemoScript.HUNT_ID)
        demo.step(10)
        demo.send(MissionCommand.Sweep(true))
        demo.step(12)
        assertThat(demo.hunt.value?.sweep?.phase).isEqualTo(SweepPhase.SWEEPING)
        assertThat(demo.pose.value?.headingDeg).isEqualTo(66.0)
        demo.step(40)
        val sweep = demo.hunt.value?.sweep
        assertThat(sweep?.phase).isEqualTo(SweepPhase.DONE)
        assertThat(sweep?.peakDeg).isEqualTo(137f)
        assertThat(sweep?.bins?.size).isEqualTo(72)
        demo.send(MissionCommand.Sweep(false))
        demo.step(41)
        assertThat(demo.hunt.value?.sweep).isNull()
    }

    @Test
    fun a_moving_target_is_announced() = runTest {
        demo.openMission(DemoScript.DF_ID)
        demo.retargets.test {
            demo.step(1)
            assertThat(awaitItem().reason).isEqualTo(RetargetReason.FIRST)
            demo.step(2)
            demo.step(DemoScript.RETARGET_TICKS)
            val moved = awaitItem()
            assertThat(moved.reason).isEqualTo(RetargetReason.MOVED)
            assertThat(moved.movedM).isGreaterThan(250.0)
        }
    }

    @Test
    fun a_survey_records_points_until_stopped() = runTest {
        demo.openMission(DemoScript.SURVEY_ID)
        demo.surveyPoints.test {
            demo.step(1)
            assertThat(awaitItem()).hasSize(1)
            demo.send(MissionCommand.StopSurvey)
            demo.step(2)
            expectNoEvents()
        }
        assertThat(demo.survey.value?.recording).isFalse()
        assertThat(demo.survey.value?.total).isEqualTo(1uL)
    }

    @Test
    fun pairing_is_refused_in_demo() = runTest {
        assertThat(demo.parsePairLink("sdrmm://pair")).isInstanceOf(Outcome.Failed::class.java)
        assertThat(demo.openMission("nope")).isInstanceOf(Outcome.Failed::class.java)
    }
}
