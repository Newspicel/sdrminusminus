package dev.newspicel.sdrmm.missions

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.WorkspaceRef
import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.Destination
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MissionsViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway().apply { servers += Samples.server() }
    private val settings = FakeSettingsStore(AppSettings(phoneName = "Pixel", activeServerId = "s1"))
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core, settings) }
    private val model by lazy { MissionsViewModel(core, settings, graph.sensors, graph.navigator, graph.router) }

    @Test
    fun sections() {
        core.missions.value =
            Samples.missions(
                Samples.mission("s", MissionKind.SURVEY, "Walk"),
                Samples.mission("r", MissionKind.RADAR_WATCH, "FM"),
                Samples.mission("d", MissionKind.DF_DRIVE, "Kraken"),
                Samples.mission("h", MissionKind.HUNT, "Beacon"),
                Samples.mission("h2", MissionKind.HUNT, "Key fob"),
            )
        val sections = model.state.value.sections
        assertThat(sections.map { it.kind }).containsExactly(MissionKind.HUNT, MissionKind.DF_DRIVE, MissionKind.RADAR_WATCH, MissionKind.SURVEY).inOrder()
        assertThat(sections.first().missions.map { it.id }).containsExactly("h", "h2").inOrder()
        assertThat(model.state.value.serverName).isEqualTo("Bench")
    }

    @Test
    fun pose_updates_do_not_reread_saved_servers() {
        assertThat(model.state.value.serverName).isEqualTo("Bench")
        val reads = core.calls.count { it == "savedServers" }
        repeat(20) { core.pose.value = Samples.pose(heading = it.toDouble()) }
        core.link.value = LinkState.Offline
        assertThat(core.calls.count { it == "savedServers" }).isEqualTo(reads)
        assertThat(model.state.value.serverName).isEqualTo("Bench")
        settings.settings.value = settings.settings.value.copy(activeServerId = null)
        assertThat(model.state.value.serverName).isEmpty()
    }

    @Test
    fun open_uses_runner() {
        val mission = Samples.mission("m1", MissionKind.HUNT)
        core.missions.value = Samples.missions(mission)
        model.open(mission)
        assertThat(core.calls).contains("openMission:m1")
        assertThat(graph.navigator.top()).isEqualTo(Destination.Mission("m1", MissionKind.HUNT))
    }

    @Test
    fun not_ready_missions_stay_closed() {
        val blocked = Samples.mission("m1").copy(ready = false, blocker = "No array")
        model.open(blocked)
        assertThat(core.calls).doesNotContain("openMission:m1")
    }

    @Test
    fun workspace_confirm() {
        val other = WorkspaceRef("w2", "Roof")
        model.askWorkspace(other)
        assertThat(model.state.value.confirmWorkspace).isEqualTo(other)
        model.confirmWorkspace()
        assertThat(model.state.value.confirmWorkspace).isNull()
        assertThat(core.calls).contains("switchWorkspace:w2")
    }

    @Test
    fun reconnect_and_refresh() {
        core.link.value = LinkState.Offline
        model.reconnect()
        model.refresh()
        assertThat(core.calls).containsAtLeast("connect:s1", "refreshMissions")
        assertThat(model.state.value.refreshing).isFalse()
    }
}
