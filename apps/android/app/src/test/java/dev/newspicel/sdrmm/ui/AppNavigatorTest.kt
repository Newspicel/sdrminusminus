package dev.newspicel.sdrmm.ui

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.mission.Banner
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeServiceControl
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import org.junit.Test

class AppNavigatorTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway()
    private val router = NoticeRouter(core, FakeSettingsStore(), Samples.retargets())
    private val runner = MissionRunner(core, FakeServiceControl())

    private fun navigator() = AppNavigator(core, runner, router)

    @Test
    fun start() {
        assertThat(navigator().backStack).containsExactly(Destination.Pair)
        core.servers += Samples.server()
        assertThat(navigator().backStack).containsExactly(Destination.Missions)
    }

    @Test
    fun unreadable_servers_start_at_pair_with_a_banner() {
        core.outcomes["savedServers"] = Outcome.Failed(CoreException.Vault(-5))
        assertThat(navigator().backStack).containsExactly(Destination.Pair)
        assertThat(router.banners.value.single()).isInstanceOf(Banner.Text::class.java)
    }

    @Test
    fun follows_runner() = runTest {
        core.servers += Samples.server()
        val hunt = Samples.mission("m1", MissionKind.HUNT)
        val df = Samples.mission("m2", MissionKind.DF_DRIVE)
        core.missions.value = Samples.missions(hunt, df)
        val navigator = navigator()
        navigator.run(backgroundScope, main.dispatcher)
        navigator.openMission(hunt)
        assertThat(navigator.backStack).containsExactly(Destination.Missions, Destination.Mission("m1", MissionKind.HUNT)).inOrder()
        runner.open("m2")
        assertThat(navigator.backStack).containsExactly(Destination.Missions, Destination.Mission("m2", MissionKind.DF_DRIVE)).inOrder()
        navigator.back()
        assertThat(navigator.backStack).containsExactly(Destination.Missions)
        assertThat(core.calls).containsAtLeast("openMission:m1", "openMission:m2", "closeMission").inOrder()
        assertThat(runner.open.value).isNull()
    }

    @Test
    fun a_mission_stopped_elsewhere_leaves_its_screen() = runTest {
        core.servers += Samples.server()
        val hunt = Samples.mission("m1", MissionKind.HUNT)
        core.missions.value = Samples.missions(hunt)
        val navigator = navigator()
        navigator.run(backgroundScope, main.dispatcher)
        navigator.openMission(hunt)
        runner.close()
        assertThat(navigator.backStack).containsExactly(Destination.Missions)
        assertThat(core.calls.count { it == "closeMission" }).isEqualTo(1)
    }

    @Test
    fun a_refused_mission_stays_closed_and_is_reported() {
        core.servers += Samples.server()
        core.outcomes["openMission"] = Outcome.Failed(CoreException.NotConnected())
        val navigator = navigator()
        navigator.openMission(Samples.mission())
        assertThat(navigator.backStack).containsExactly(Destination.Missions)
        assertThat(router.banners.value).hasSize(1)
    }

    @Test
    fun pairing_replaces_the_stack() {
        val navigator = navigator()
        navigator.push(Destination.Settings)
        navigator.paired()
        assertThat(navigator.backStack).containsExactly(Destination.Missions)
        navigator.showPair()
        navigator.showPair()
        assertThat(navigator.backStack).containsExactly(Destination.Missions, Destination.Pair).inOrder()
        navigator.toPair()
        assertThat(navigator.backStack).containsExactly(Destination.Pair)
    }
}
