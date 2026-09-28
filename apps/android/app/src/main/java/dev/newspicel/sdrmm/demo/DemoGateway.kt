package dev.newspicel.sdrmm.demo

import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreAbout
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.HeadingSample
import dev.newspicel.sdrmm.ffi.HuntView
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LicenseEntry
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.LocationSample
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionKind
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
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

class DemoGateway(
    private val base: CoreGateway?,
    private val tickMs: Long = TICK_MS,
) : CoreGateway {
    private val script = DemoScript()
    private val linkState = MutableStateFlow<LinkState>(LinkState.Online(SERVER_NAME))
    private val missionsState = MutableStateFlow<MissionsView?>(script.missions())
    private val huntState = MutableStateFlow<HuntView?>(null)
    private val dfState = MutableStateFlow<DfView?>(null)
    private val radarState = MutableStateFlow<RadarView?>(null)
    private val imageState = MutableStateFlow<RgbaImage?>(null)
    private val openId = MutableStateFlow<String?>(null)
    private val running = MutableStateFlow(true)

    override val link: StateFlow<LinkState> = linkState.asStateFlow()
    override val missions: StateFlow<MissionsView?> = missionsState.asStateFlow()
    override val pose: StateFlow<PoseView?> = MutableStateFlow(script.pose()).asStateFlow()
    override val hunt: StateFlow<HuntView?> = huntState.asStateFlow()
    override val df: StateFlow<DfView?> = dfState.asStateFlow()
    override val radar: StateFlow<RadarView?> = radarState.asStateFlow()
    override val radarImage: StateFlow<RgbaImage?> = imageState.asStateFlow()
    override val survey: StateFlow<SurveyView?> = MutableStateFlow<SurveyView?>(null).asStateFlow()
    override val surveyPoints: SharedFlow<List<SurveyPoint>> = MutableSharedFlow<List<SurveyPoint>>().asSharedFlow()
    override val retargets: SharedFlow<RetargetNotice> = MutableSharedFlow<RetargetNotice>().asSharedFlow()
    override val notices: SharedFlow<Notice> = MutableSharedFlow<Notice>().asSharedFlow()
    override val missedUpdates: StateFlow<Long> = MutableStateFlow(0L).asStateFlow()

    override fun run(scope: CoroutineScope): Job = scope.launch {
        var tick = 0L
        while (isActive) {
            step(tick)
            tick += 1
            delay(tickMs)
        }
    }

    fun step(tick: Long) {
        when (script.kindOf(openId.value)) {
            MissionKind.HUNT -> {
                huntState.value = script.hunt(tick, running.value)
            }

            MissionKind.DF_DRIVE -> {
                dfState.value = script.df(tick)
            }

            MissionKind.RADAR_WATCH -> {
                radarState.value = script.radar(tick)
                imageState.value = script.image(tick)
            }

            else -> {
                Unit
            }
        }
    }

    override fun about(): CoreAbout = base?.about() ?: CoreAbout(coreVersion = "-", protocol = 0u)

    override fun licenses(): List<LicenseEntry> = base?.licenses() ?: emptyList()

    override fun savedServers(): Outcome<List<SavedServer>> = Outcome.Ok(listOf(script.server()))

    override fun parsePairLink(link: String): Outcome<PairOffer> = refused()

    override fun offerFromDiscovery(
        server: DiscoveredServer,
        code: String,
    ): Outcome<PairOffer> = refused()

    override suspend fun offerManual(
        address: String,
        code: String,
    ): Outcome<PairOffer> = refused()

    override suspend fun pair(
        offer: PairOffer,
        phoneName: String,
    ): Outcome<SavedServer> = refused()

    override fun forgetServer(id: String): Outcome<Unit> = refused()

    override fun updateHosts(
        serverId: String,
        hosts: List<String>,
    ): Outcome<Unit> = Outcome.Ok(Unit)

    override suspend fun connect(serverId: String): Outcome<Unit> {
        linkState.value = LinkState.Online(SERVER_NAME)
        return Outcome.Ok(Unit)
    }

    override fun disconnect() {
        linkState.value = LinkState.Offline
    }

    override fun setForeground(foreground: Boolean) = Unit

    override fun networkChanged() = Unit

    override fun setLocalNetworkAllowed(allowed: Boolean) = Unit

    override suspend fun refreshMissions(): Outcome<Unit> {
        missionsState.value = script.missions()
        return Outcome.Ok(Unit)
    }

    override suspend fun switchWorkspace(id: String): Outcome<Unit> = refused()

    override fun openMission(id: String): Outcome<Unit> {
        if (script.kindOf(id) == null) return Outcome.Failed(CoreException.NoMission())
        openId.value = id
        return Outcome.Ok(Unit)
    }

    override fun closeMission() {
        openId.value = null
    }

    override suspend fun send(command: MissionCommand): Outcome<Unit> {
        when (command) {
            MissionCommand.StartHunt -> running.value = true
            MissionCommand.StopHunt -> running.value = false
            is MissionCommand.Tune -> script.tune(command.hz)
            else -> Unit
        }
        huntState.update { it?.copy(running = running.value) }
        return Outcome.Ok(Unit)
    }

    override fun pushLocation(sample: LocationSample) = Unit

    override fun pushHeading(sample: HeadingSample) = Unit

    override fun pushMotion(sample: MotionSample) = Unit

    override fun setPoseSettings(settings: PoseSettings) = Unit

    override fun startAlign() = Unit

    override fun cancelAlign() = Unit

    override fun navUri(
        target: LatLon,
        app: NavApp,
    ): String = base?.navUri(target, app) ?: "geo:${target.lat},${target.lon}"

    override fun distanceM(
        from: LatLon,
        to: LatLon,
    ): Double = base?.distanceM(from, to) ?: DemoScript.distanceM(from, to)

    override fun bearingDeg(
        from: LatLon,
        to: LatLon,
    ): Double = base?.bearingDeg(from, to) ?: DemoScript.bearingDeg(from, to)

    private fun <T> refused(): Outcome<T> = Outcome.Failed(CoreException.Refused(DEMO_REFUSAL))

    companion object {
        const val SERVER_ID = "demo"
        const val SERVER_NAME = "Demo"
        private const val DEMO_REFUSAL = "Not in demo"
        private const val TICK_MS = 500L
    }
}
