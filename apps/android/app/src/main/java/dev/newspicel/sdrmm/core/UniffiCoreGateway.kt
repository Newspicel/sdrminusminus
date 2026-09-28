package dev.newspicel.sdrmm.core

import android.content.Context
import android.os.Build
import dev.newspicel.sdrmm.BuildConfig
import dev.newspicel.sdrmm.ffi.CoreAbout
import dev.newspicel.sdrmm.ffi.CoreConfig
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
import dev.newspicel.sdrmm.ffi.MobileCore
import dev.newspicel.sdrmm.ffi.MotionSample
import dev.newspicel.sdrmm.ffi.NavApp
import dev.newspicel.sdrmm.ffi.Notice
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.ffi.Platform
import dev.newspicel.sdrmm.ffi.PoseSettings
import dev.newspicel.sdrmm.ffi.PoseView
import dev.newspicel.sdrmm.ffi.RadarView
import dev.newspicel.sdrmm.ffi.RetargetNotice
import dev.newspicel.sdrmm.ffi.RgbaImage
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.ffi.SecretVault
import dev.newspicel.sdrmm.ffi.SurveyPoint
import dev.newspicel.sdrmm.ffi.SurveyView
import dev.newspicel.sdrmm.ffi.geoBearingDeg
import dev.newspicel.sdrmm.ffi.geoDistanceM
import dev.newspicel.sdrmm.ffi.navHandoffUri
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow

class UniffiCoreGateway(
    private val core: MobileCore,
    private val hub: CoreStateHub,
) : CoreGateway {
    override val link: StateFlow<LinkState> = hub.link.asStateFlow()
    override val missions: StateFlow<MissionsView?> = hub.missions.asStateFlow()
    override val pose: StateFlow<PoseView?> = hub.pose.asStateFlow()
    override val hunt: StateFlow<HuntView?> = hub.hunt.asStateFlow()
    override val df: StateFlow<DfView?> = hub.df.asStateFlow()
    override val radar: StateFlow<RadarView?> = hub.radar.asStateFlow()
    override val radarImage: StateFlow<RgbaImage?> = hub.radarImage.asStateFlow()
    override val survey: StateFlow<SurveyView?> = hub.survey.asStateFlow()
    override val surveyPoints: SharedFlow<List<SurveyPoint>> = hub.surveyPoints.asSharedFlow()
    override val retargets: SharedFlow<RetargetNotice> = hub.retargets.asSharedFlow()
    override val notices: SharedFlow<Notice> = hub.notices.asSharedFlow()
    override val missedUpdates: StateFlow<Long> = hub.missedUpdates.asStateFlow()

    override fun run(scope: CoroutineScope): Job = CoreEventPump(core, hub).run(scope)

    override fun about(): CoreAbout = core.about()

    override fun licenses(): List<LicenseEntry> = core.notices()

    override fun savedServers(): Outcome<List<SavedServer>> = catching { core.savedServers() }

    override fun parsePairLink(link: String): Outcome<PairOffer> = catching { core.parsePairLink(link) }

    override fun offerFromDiscovery(
        server: DiscoveredServer,
        code: String,
    ): Outcome<PairOffer> = catching { core.offerFromDiscovery(server, code) }

    override suspend fun offerManual(
        address: String,
        code: String,
    ): Outcome<PairOffer> = catching { core.offerManual(address, code) }

    override suspend fun pair(
        offer: PairOffer,
        phoneName: String,
    ): Outcome<SavedServer> = catching { core.pair(offer, phoneName) }

    override fun forgetServer(id: String): Outcome<Unit> = catching { core.forgetServer(id) }

    override fun updateHosts(
        serverId: String,
        hosts: List<String>,
    ): Outcome<Unit> = catching { core.updateHosts(serverId, hosts) }

    override suspend fun connect(serverId: String): Outcome<Unit> = catching { core.connect(serverId) }

    override fun disconnect() = core.disconnect()

    override fun setForeground(foreground: Boolean) = core.setForeground(foreground)

    override fun networkChanged() = core.networkChanged()

    override fun setLocalNetworkAllowed(allowed: Boolean) = core.setLocalNetworkAllowed(allowed)

    override suspend fun refreshMissions(): Outcome<Unit> = catching { core.refreshMissions() }

    override suspend fun switchWorkspace(id: String): Outcome<Unit> = catching { core.switchWorkspace(id) }

    override fun openMission(id: String): Outcome<Unit> = catching { core.openMission(id) }

    override fun closeMission() = core.closeMission()

    override suspend fun send(command: MissionCommand): Outcome<Unit> = catching { core.send(command) }

    override fun pushLocation(sample: LocationSample) = core.pushLocation(sample)

    override fun pushHeading(sample: HeadingSample) = core.pushHeading(sample)

    override fun pushMotion(sample: MotionSample) = core.pushMotion(sample)

    override fun setPoseSettings(settings: PoseSettings) = core.setPoseSettings(settings)

    override fun startAlign() = core.startAlign()

    override fun cancelAlign() = core.cancelAlign()

    override fun navUri(
        target: LatLon,
        app: NavApp,
    ): String = navHandoffUri(target, app)

    override fun distanceM(
        from: LatLon,
        to: LatLon,
    ): Double = geoDistanceM(from, to)

    override fun bearingDeg(
        from: LatLon,
        to: LatLon,
    ): Double = geoBearingDeg(from, to)

    companion object {
        fun create(
            context: Context,
            vault: SecretVault,
        ): Outcome<UniffiCoreGateway> = catching {
            val config =
                CoreConfig(
                    appVersion = BuildConfig.VERSION_NAME,
                    platform = Platform.ANDROID,
                    deviceModel = Build.MODEL,
                    dataDir = context.noBackupFilesDir.path,
                )
            UniffiCoreGateway(MobileCore(config, vault), CoreStateHub())
        }
    }
}
