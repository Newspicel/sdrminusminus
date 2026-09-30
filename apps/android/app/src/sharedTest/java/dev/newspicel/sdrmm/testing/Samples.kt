package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.ffi.AlignState
import dev.newspicel.sdrmm.ffi.DfOverlay
import dev.newspicel.sdrmm.ffi.DfState
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.EstimateView
import dev.newspicel.sdrmm.ffi.GuidanceKind
import dev.newspicel.sdrmm.ffi.GuidanceView
import dev.newspicel.sdrmm.ffi.HeadingSourceKind
import dev.newspicel.sdrmm.ffi.HeatBand
import dev.newspicel.sdrmm.ffi.HuntView
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LicenseEntry
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.ffi.MissionsView
import dev.newspicel.sdrmm.ffi.NavPoint
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.RadarTrack
import dev.newspicel.sdrmm.ffi.Ray
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RetargetReason
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.ffi.Station
import dev.newspicel.sdrmm.ffi.SweepPhase
import dev.newspicel.sdrmm.ffi.SweepView
import dev.newspicel.sdrmm.ffi.TargetMode
import dev.newspicel.sdrmm.ffi.Trend
import dev.newspicel.sdrmm.ffi.WorkspaceRef
import dev.newspicel.sdrmm.mission.Retargets
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

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
        controls: List<MissionControl> = listOf(MissionControl.HUNT_RUN),
    ): Mission = Mission(id, kind, title, "433.920 MHz", true, null, controls)

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

    fun retargets(
        lastFix: StateFlow<LocationSample?> = MutableStateFlow(null),
        handedOff: StateFlow<Set<String>> = MutableStateFlow(emptySet()),
        appResumed: StateFlow<Boolean> = MutableStateFlow(true),
        speak: (String) -> Unit = {},
        notify: (RetargetNotice, String) -> Unit = { _, _ -> },
    ): Retargets = Retargets(lastFix, handedOff, appResumed, speak, notify)

    fun fix(
        lat: Double = 52.52,
        lon: Double = 13.405,
    ): LocationSample = LocationSample(1_000L, lat, lon, null, 5.0, null, null, null, null, null)

    fun hunt(
        mission: String = "m1",
        running: Boolean = false,
        trend: Trend = Trend.WARMER,
        strength: Float = 0.5f,
        sweep: SweepView? = null,
    ): HuntView = HuntView(mission, 145_500_000.0, -60f, -61f, -90f, -55f, strength, trend, running, null, 12uL, sweep)

    fun sweep(
        bins: ByteArray = ByteArray(72) { (it * 3).toByte() },
        peak: Float? = 137f,
        phase: SweepPhase = SweepPhase.DONE,
    ): SweepView = SweepView(bins, peak, 270f, phase, 8f)

    fun df(
        mission: String = "d1",
        state: DfState = DfState.LIVE,
        guidance: GuidanceView? = GuidanceView(GuidanceKind.PROBE, 215.0, -40.0, 1200.0),
        target: NavPoint? = NavPoint(LatLon(52.52, 13.405), GuidanceKind.PROBE),
        bearingTrue: Float? = 137f,
        bearingRel: Float? = 30f,
        heat: List<HeatBand> = emptyList(),
    ): DfView = DfView(
        mission,
        state,
        bearingTrue,
        bearingRel,
        0.62f,
        6f,
        145_500_000.0,
        TargetMode.AUTO,
        guidance,
        target,
        EstimateView(LatLon(52.53, 13.41), 300.0, 120.0, 40.0, false, 12u),
        DfOverlay(
            rays = listOf(Ray(LatLon(52.5, 13.4), LatLon(52.55, 13.45), 0.8f)),
            stations = listOf(Station("s1", LatLon(52.5, 13.4), 4u)),
            ellipse = emptyList(),
            heat = heat,
        ),
    )

    fun track(
        id: Int,
        snr: Float,
        doppler: Float = -35f,
        closing: Boolean = true,
    ): RadarTrack = RadarTrack(id.toUInt(), 12.4f, doppler, 10f, snr, closing, false, null)

    fun notice(
        reason: RetargetReason = RetargetReason.MOVED,
        mission: String = "d1",
        kind: GuidanceKind = GuidanceKind.ESTIMATE,
        at: LatLon = LatLon(52.53, 13.42),
    ): RetargetNotice = RetargetNotice(mission, NavPoint(at, kind), 400.0, reason)

    fun license(): LicenseEntry = LicenseEntry("uniffi", "0.32.2", "MPL-2.0", "Mozilla Public License 2.0")
}
