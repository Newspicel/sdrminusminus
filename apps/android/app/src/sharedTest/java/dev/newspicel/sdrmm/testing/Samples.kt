package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.ffi.AlignState
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.HeadingSourceKind
import dev.newspicel.sdrmm.ffi.LicenseEntry
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.MissionsView
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.ffi.WorkspaceRef

object Samples {
    const val PIN = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0"
    const val KEY_CHECK = "AB12 CD34 EF56 GH78 JK90"

    fun link(code: String = "48210937"): String = "sdrmm://pair?h=192.168.1.20%3A8443&c=$code&fp=$PIN&p=3&n=Bench"

    fun offer(
        hosts: List<String> = listOf("192.168.1.20:8443"),
        code: String = "48210937",
    ): PairOffer = PairOffer(
        hosts = hosts,
        code = code,
        fingerprint = PIN,
        fingerprintShort = KEY_CHECK,
        protocol = 3u,
        serverName = "Bench",
    )

    fun server(
        id: String = "s1",
        name: String = "Bench",
    ): SavedServer = SavedServer(
        id = id,
        name = name,
        hosts = listOf("192.168.1.20:8443"),
        fingerprintShort = KEY_CHECK,
        phoneId = "0123456789abcdef",
        pairedAt = "2026-09-28T12:00:00Z",
    )

    fun discovered(
        name: String = "SDR-- bench",
        id: String = "s1",
    ): DiscoveredServer = DiscoveredServer(
        name = name,
        hosts = listOf("192.168.1.20:8443"),
        txt = mapOf("v" to "1", "p" to "3", "id" to id, "fp" to PIN),
    )

    fun mission(
        id: String = "m1",
        kind: MissionKind = MissionKind.HUNT,
        title: String = "Beacon",
    ): Mission = Mission(id, kind, title, "433.920 MHz", true, null, listOf(MissionControl.HUNT_RUN))

    fun missions(vararg missions: Mission): MissionsView {
        val workspace = WorkspaceRef("w1", "Field")
        return MissionsView(workspace, listOf(workspace), missions.toList())
    }

    fun pose(
        heading: Double? = 42.0,
        accuracy: Double? = 12.0,
        source: HeadingSourceKind = HeadingSourceKind.COMPASS,
        sending: Boolean = false,
    ): PoseView = PoseView(heading, accuracy, source, AlignState.Idle, sending, null)

    fun license(): LicenseEntry = LicenseEntry("uniffi", "0.32.2", "MPL-2.0", "Mozilla Public License 2.0")
}
