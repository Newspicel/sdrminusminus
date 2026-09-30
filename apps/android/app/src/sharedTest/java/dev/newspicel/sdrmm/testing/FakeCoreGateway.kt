package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreAbout
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.HeadingSample
import dev.newspicel.sdrmm.ffi.HuntView
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LicenseEntry
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionsView
import dev.newspicel.sdrmm.ffi.MotionSample
import dev.newspicel.sdrmm.ffi.NavApp
import dev.newspicel.sdrmm.ffi.Notice
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.ffi.PoseSettings
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.ffi.SurveyPoint
import dev.newspicel.sdrmm.ffi.SurveyView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import java.util.Collections

class FakeCoreGateway : CoreGateway {
    override val link = MutableStateFlow<LinkState>(LinkState.Offline)
    override val missions = MutableStateFlow<MissionsView?>(null)
    override val pose = MutableStateFlow<PoseView?>(null)
    override val hunt = MutableStateFlow<HuntView?>(null)
    override val df = MutableStateFlow<DfView?>(null)
    override val radar = MutableStateFlow<RadarView?>(null)
    override val radarImage = MutableStateFlow<RgbaImage?>(null)
    override val survey = MutableStateFlow<SurveyView?>(null)
    override val surveyPoints = MutableSharedFlow<List<SurveyPoint>>(extraBufferCapacity = 64)
    override val retargets = MutableSharedFlow<RetargetNotice>(extraBufferCapacity = 64)
    override val notices = MutableSharedFlow<Notice>(extraBufferCapacity = 64)
    override val missedUpdates = MutableStateFlow(0L)

    val calls: MutableList<String> = Collections.synchronizedList(mutableListOf())
    val outcomes: MutableMap<String, Outcome<*>> = Collections.synchronizedMap(mutableMapOf())
    var servers: MutableList<SavedServer> = mutableListOf()
    var about = CoreAbout(coreVersion = "0.0.0-test", protocol = 3u)
    var licenseEntries = listOf(Samples.license())
    val locations: MutableList<LocationSample> = Collections.synchronizedList(mutableListOf())
    val headings: MutableList<HeadingSample> = Collections.synchronizedList(mutableListOf())
    val motions: MutableList<MotionSample> = Collections.synchronizedList(mutableListOf())
    val poseSettings: MutableList<PoseSettings> = Collections.synchronizedList(mutableListOf())
    val localNetwork: MutableList<Boolean> = Collections.synchronizedList(mutableListOf())
    val commands: MutableList<MissionCommand> = Collections.synchronizedList(mutableListOf())

    @Suppress("UNCHECKED_CAST")
    private fun <T> scripted(
        call: String,
        default: T,
    ): Outcome<T> {
        calls += call
        val key = call.substringBefore(':')
        return (outcomes[call] ?: outcomes[key]) as Outcome<T>? ?: Outcome.Ok(default)
    }

    override fun run(scope: CoroutineScope): Job = Job().apply { complete() }

    override fun about(): CoreAbout = about

    override fun licenses(): List<LicenseEntry> = licenseEntries

    override fun savedServers(): Outcome<List<SavedServer>> = scripted("savedServers", servers.toList())

    override fun parsePairLink(link: String): Outcome<PairOffer> = scripted("parsePairLink:$link", Samples.offer())

    override fun offerFromDiscovery(
        server: DiscoveredServer,
        code: String,
    ): Outcome<PairOffer> = scripted("offerFromDiscovery:${server.name}", Samples.offer(code = code))

    override suspend fun offerManual(
        address: String,
        code: String,
    ): Outcome<PairOffer> = scripted("offerManual:$address", Samples.offer(hosts = listOf(address), code = code))

    override suspend fun pair(
        offer: PairOffer,
        phoneName: String,
    ): Outcome<SavedServer> {
        val paired = scripted("pair:$phoneName", Samples.server())
        if (paired is Outcome.Ok) servers += paired.value
        return paired
    }

    override fun forgetServer(id: String): Outcome<Unit> {
        val forgotten = scripted("forgetServer:$id", Unit)
        if (forgotten is Outcome.Ok) servers.removeAll { it.id == id }
        return forgotten
    }

    override fun updateHosts(
        serverId: String,
        hosts: List<String>,
    ): Outcome<Unit> = scripted("updateHosts:$serverId", Unit)

    override suspend fun connect(serverId: String): Outcome<Unit> = scripted("connect:$serverId", Unit)

    override fun disconnect() {
        calls += "disconnect"
    }

    override fun setForeground(foreground: Boolean) {
        calls += "setForeground:$foreground"
    }

    override fun networkChanged() {
        calls += "networkChanged"
    }

    override fun setLocalNetworkAllowed(allowed: Boolean) {
        localNetwork += allowed
    }

    override suspend fun refreshMissions(): Outcome<Unit> = scripted("refreshMissions", Unit)

    override suspend fun switchWorkspace(id: String): Outcome<Unit> = scripted("switchWorkspace:$id", Unit)

    override fun openMission(id: String): Outcome<Unit> = scripted("openMission:$id", Unit)

    override fun closeMission() {
        calls += "closeMission"
    }

    override suspend fun send(command: MissionCommand): Outcome<Unit> {
        commands += command
        return scripted("send:${command.javaClass.simpleName}", Unit)
    }

    override fun pushLocation(sample: LocationSample) {
        locations += sample
    }

    override fun pushHeading(sample: HeadingSample) {
        headings += sample
    }

    override fun pushMotion(sample: MotionSample) {
        motions += sample
    }

    override fun setPoseSettings(settings: PoseSettings) {
        poseSettings += settings
    }

    override fun startAlign() {
        calls += "startAlign"
    }

    override fun cancelAlign() {
        calls += "cancelAlign"
    }

    override fun navUri(
        target: LatLon,
        app: NavApp,
    ): String = String.format(java.util.Locale.ROOT, "geo:%.6f,%.6f", target.lat, target.lon)

    override fun distanceM(
        from: LatLon,
        to: LatLon,
    ): Double = dev.newspicel.sdrmm.demo.DemoScript
        .distanceM(from, to)

    override fun bearingDeg(
        from: LatLon,
        to: LatLon,
    ): Double = dev.newspicel.sdrmm.demo.DemoScript
        .bearingDeg(from, to)
}
