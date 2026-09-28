package dev.newspicel.sdrmm.core

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
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow

interface CoreGateway {
    val link: StateFlow<LinkState>
    val missions: StateFlow<MissionsView?>
    val pose: StateFlow<PoseView?>
    val hunt: StateFlow<HuntView?>
    val df: StateFlow<DfView?>
    val radar: StateFlow<RadarView?>
    val radarImage: StateFlow<RgbaImage?>
    val survey: StateFlow<SurveyView?>
    val surveyPoints: SharedFlow<List<SurveyPoint>>
    val retargets: SharedFlow<RetargetNotice>
    val notices: SharedFlow<Notice>
    val missedUpdates: StateFlow<Long>

    fun run(scope: CoroutineScope): Job

    fun about(): CoreAbout

    fun licenses(): List<LicenseEntry>

    fun savedServers(): Outcome<List<SavedServer>>

    fun parsePairLink(link: String): Outcome<PairOffer>

    fun offerFromDiscovery(
        server: DiscoveredServer,
        code: String,
    ): Outcome<PairOffer>

    suspend fun offerManual(
        address: String,
        code: String,
    ): Outcome<PairOffer>

    suspend fun pair(
        offer: PairOffer,
        phoneName: String,
    ): Outcome<SavedServer>

    fun forgetServer(id: String): Outcome<Unit>

    fun updateHosts(
        serverId: String,
        hosts: List<String>,
    ): Outcome<Unit>

    suspend fun connect(serverId: String): Outcome<Unit>

    fun disconnect()

    fun setForeground(foreground: Boolean)

    fun networkChanged()

    fun setLocalNetworkAllowed(allowed: Boolean)

    suspend fun refreshMissions(): Outcome<Unit>

    suspend fun switchWorkspace(id: String): Outcome<Unit>

    fun openMission(id: String): Outcome<Unit>

    fun closeMission()

    suspend fun send(command: MissionCommand): Outcome<Unit>

    fun pushLocation(sample: LocationSample)

    fun pushHeading(sample: HeadingSample)

    fun pushMotion(sample: MotionSample)

    fun setPoseSettings(settings: PoseSettings)

    fun startAlign()

    fun cancelAlign()

    fun navUri(
        target: LatLon,
        app: NavApp,
    ): String

    fun distanceM(
        from: LatLon,
        to: LatLon,
    ): Double

    fun bearingDeg(
        from: LatLon,
        to: LatLon,
    ): Double
}
