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
import dev.newspicel.sdrmm.ffi.NavPoint
import dev.newspicel.sdrmm.ffi.Notice
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.ffi.PoseSettings
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RetargetReason
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
    private val poseState = MutableStateFlow<PoseView?>(script.pose())
    private val surveyState = MutableStateFlow<SurveyView?>(null)
    private val pointsFlow = MutableSharedFlow<List<SurveyPoint>>(extraBufferCapacity = BUFFER)
    private val retargetFlow = MutableSharedFlow<RetargetNotice>(extraBufferCapacity = BUFFER)
    private val announced = MutableStateFlow<NavPoint?>(null)
    private val sweepFrom = MutableStateFlow<Long?>(null)
    private val recording = MutableStateFlow(true)
    private val surveyed = MutableStateFlow(0L)
    private var lastTick = 0L

    override val link: StateFlow<LinkState> = linkState.asStateFlow()
    override val missions: StateFlow<MissionsView?> = missionsState.asStateFlow()
    override val pose: StateFlow<PoseView?> = poseState.asStateFlow()
    override val hunt: StateFlow<HuntView?> = huntState.asStateFlow()
    override val df: StateFlow<DfView?> = dfState.asStateFlow()
    override val radar: StateFlow<RadarView?> = radarState.asStateFlow()
    override val radarImage: StateFlow<RgbaImage?> = imageState.asStateFlow()
    override val survey: StateFlow<SurveyView?> = surveyState.asStateFlow()
    override val surveyPoints: SharedFlow<List<SurveyPoint>> = pointsFlow.asSharedFlow()
    override val retargets: SharedFlow<RetargetNotice> = retargetFlow.asSharedFlow()
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
        lastTick = tick
        when (script.kindOf(openId.value)) {
            MissionKind.HUNT -> hunt(tick)

            MissionKind.DF_DRIVE -> df(tick)

            MissionKind.RADAR_WATCH -> {
                radarState.value = script.radar(tick)
                imageState.value = script.image(tick)
            }

            MissionKind.SURVEY -> survey(tick)

            null -> Unit
        }
    }

    private fun hunt(tick: Long) {
        val from = sweepFrom.value
        val sweep = from?.let { script.sweep(tick - it) }
        poseState.value = script.pose(from?.let { script.sweepHeading(tick - it) } ?: script.pose().headingDeg ?: 0.0)
        huntState.value = script.hunt(tick, running.value, sweep)
    }

    private fun df(tick: Long) {
        val view = script.df(tick)
        dfState.value = view
        val target = view.target ?: return
        val last = announced.value
        if (last == target) return
        announced.value = target
        val reason = if (last == null) RetargetReason.FIRST else RetargetReason.MOVED
        val moved = last?.let { DemoScript.distanceM(it.at, target.at) } ?: 0.0
        retargetFlow.tryEmit(RetargetNotice(DemoScript.DF_ID, target, moved, reason))
    }

    private fun survey(tick: Long) {
        val point = script.surveyPoint(tick)
        if (recording.value) {
            surveyed.value += 1
            pointsFlow.tryEmit(listOf(point))
        }
        surveyState.value = script.survey(point.levelDb, surveyed.value, recording.value)
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

            is MissionCommand.Sweep -> {
                sweepFrom.value = if (command.on) lastTick else null
                if (command.on) running.value = true
            }

            MissionCommand.StartSurvey -> recording.value = true

            MissionCommand.StopSurvey -> recording.value = false

            MissionCommand.ClearSurvey -> surveyed.value = 0

            MissionCommand.Mark, MissionCommand.Calibrate, MissionCommand.ClearFusion, is MissionCommand.SetTargetMode -> Unit
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
        private const val BUFFER = 64
    }
}
