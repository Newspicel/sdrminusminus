package dev.newspicel.sdrmm.demo

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionKind
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
        ).containsExactly(MissionKind.HUNT, MissionKind.DF_DRIVE, MissionKind.RADAR_WATCH)
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
    fun pairing_is_refused_in_demo() = runTest {
        assertThat(demo.parsePairLink("sdrmm://pair")).isInstanceOf(Outcome.Failed::class.java)
        assertThat(demo.openMission("nope")).isInstanceOf(Outcome.Failed::class.java)
    }
}
