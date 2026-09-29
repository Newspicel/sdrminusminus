package dev.newspicel.sdrmm

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import androidx.car.app.connection.CarConnection
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.Observer
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.lifecycle.ViewModelStore
import dev.newspicel.sdrmm.audio.ClickController
import dev.newspicel.sdrmm.audio.ClickTrack
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.ffi.RefusalKind
import dev.newspicel.sdrmm.haptics.Haptics
import dev.newspicel.sdrmm.mission.AlertNotifier
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.mission.Retargets
import dev.newspicel.sdrmm.mission.ServiceControl
import dev.newspicel.sdrmm.nav.NavHandoff
import dev.newspicel.sdrmm.pair.Discovery
import dev.newspicel.sdrmm.pair.PairLinkIntake
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionPlan
import dev.newspicel.sdrmm.sensors.Declination
import dev.newspicel.sdrmm.sensors.GeomagneticDeclination
import dev.newspicel.sdrmm.sensors.Holder
import dev.newspicel.sdrmm.sensors.SensorHub
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.settings.syncPoseSettings
import dev.newspicel.sdrmm.speech.Speaker
import dev.newspicel.sdrmm.survey.SurveyTrailStore
import dev.newspicel.sdrmm.ui.AppNavigator
import dev.newspicel.sdrmm.ui.components.UiText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

sealed interface Startup {
    data class Ready(
        val graph: AppGraph,
    ) : Startup

    data class Failed(
        val message: String,
    ) : Startup
}

class Outputs(
    val speaker: Speaker,
    val clickTrack: ClickTrack,
    val haptics: Haptics,
    val service: ServiceControl,
)

fun interface OutputFactory {
    fun create(
        app: Application,
        settings: SettingsStore,
        scope: CoroutineScope,
    ): Outputs
}

class AppGraph(
    val app: Application,
    val core: CoreGateway,
    val settings: SettingsStore,
    val sensors: SensorHub,
    val runner: MissionRunner,
    val router: NoticeRouter,
    val navigator: AppNavigator,
    val intake: PairLinkIntake,
    val discovery: Discovery,
    val demo: DemoSwitch,
    val nav: NavHandoff,
    val speaker: Speaker,
    val clickTrack: ClickTrack,
    val clicks: ClickController,
    val haptics: Haptics,
    val alerts: AlertNotifier,
    val surveyTrail: SurveyTrailStore,
    val scope: CoroutineScope,
    private val resumed: MutableStateFlow<Boolean>,
    private val car: MutableStateFlow<Boolean>,
) {
    val viewModels = ViewModelStore()
    val appResumed: StateFlow<Boolean> = resumed.asStateFlow()
    val carConnected: StateFlow<Boolean> = car.asStateFlow()
    private val carConnection = CarConnection(app)
    private val carType = Observer<Int> { car.value = it == CarConnection.CONNECTION_TYPE_PROJECTION }

    private val lifecycle =
        object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                core.setForeground(true)
                sensors.acquire(Holder.App)
            }

            override fun onResume(owner: LifecycleOwner) {
                resumed.value = true
            }

            override fun onPause(owner: LifecycleOwner) {
                resumed.value = false
            }

            override fun onStop(owner: LifecycleOwner) {
                core.setForeground(false)
                sensors.release(Holder.App)
            }
        }

    private val network =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                core.networkChanged()
            }
        }

    fun start() {
        router.run(scope)
        core.run(scope)
        permissionsChanged()
        syncPoseSettings(scope, settings, core)
        scope.launch { connectActive() }
        scope.launch {
            core.link
                .filter { it is LinkState.Refused && it.reason == RefusalKind.REVOKED }
                .collect { revoked() }
        }
        navigator.run(scope)
        clicks.run(scope)
        surveyTrail.run(scope)
        scope.launch { rediscover() }
        app.getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(network)
        ProcessLifecycleOwner.get().lifecycle.addObserver(lifecycle)
        carConnection.type.observeForever(carType)
    }

    fun stop() {
        carConnection.type.removeObserver(carType)
        ProcessLifecycleOwner.get().lifecycle.removeObserver(lifecycle)
        sensors.release(Holder.App)
        app.getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(network)
        if (runner.open.value != null) runner.close()
        clickTrack.stop()
        speaker.shutdown()
        viewModels.clear()
        scope.cancel()
    }

    fun activate(serverId: String) {
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            settings.update { it.copy(activeServerId = serverId) }
            val connected = core.connect(serverId)
            if (connected is Outcome.Failed) router.report(connected.error)
        }
    }

    fun permissionsChanged() {
        core.setLocalNetworkAllowed(PermissionPlan.granted(app, Need.LocalNetwork))
        sensors.permissionsChanged()
    }

    fun missionTitle(id: String): String = core.missions.value
        ?.missions
        ?.firstOrNull { it.id == id }
        ?.title ?: id

    private suspend fun connectActive() {
        val id = settings.current().activeServerId ?: return
        val connected = core.connect(id)
        if (connected is Outcome.Failed) router.report(connected.error)
    }

    private suspend fun rediscover() {
        core.link
            .map { it is LinkState.Connecting && it.attempt >= REDISCOVER_AFTER }
            .distinctUntilChanged()
            .collectLatest { searching ->
                if (!searching) return@collectLatest
                discovery.start()
                try {
                    discovery.found.collect(::refreshHosts)
                } finally {
                    discovery.stop()
                }
            }
    }

    private fun refreshHosts(found: List<DiscoveredServer>) {
        val saved = (core.savedServers() as? Outcome.Ok)?.value?.map { it.id }?.toSet() ?: return
        for (server in found) {
            val id = server.txt[TXT_SERVER_ID] ?: continue
            if (id !in saved) continue
            val updated = core.updateHosts(id, server.hosts)
            if (updated is Outcome.Failed) router.report(updated.error)
        }
    }

    private suspend fun revoked() {
        router.show(NoticeLevel.ERROR, UiText.Res(R.string.err_revoked))
        val id = settings.current().activeServerId
        if (id != null) {
            val forgotten = core.forgetServer(id)
            if (forgotten is Outcome.Failed) router.report(forgotten.error)
        }
        settings.update { it.copy(activeServerId = null) }
        launchMain { navigator.toPair() }
    }

    private fun launchMain(block: () -> Unit) {
        scope.launch(Dispatchers.Main.immediate) { block() }
    }

    companion object {
        private val REDISCOVER_AFTER = 2u
        private const val TXT_SERVER_ID = "id"

        fun assemble(
            app: Application,
            core: CoreGateway,
            settings: SettingsStore,
            demo: DemoSwitch,
            discovery: Discovery,
            outputs: OutputFactory,
        ): AppGraph {
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val made = outputs.create(app, settings, scope)
            val resumed = MutableStateFlow(false)
            val car = MutableStateFlow(false)
            val declination = Declination(GeomagneticDeclination(), System::currentTimeMillis)
            val sensors = SensorHub(app, core, settings, declination, scope)
            val nav = NavHandoff(app, core, settings)
            val alerts = AlertNotifier(app, core)
            val retargets =
                Retargets(
                    lastFix = sensors.lastFix,
                    handedOff = nav.handedOff,
                    appResumed = resumed,
                    speak = { text -> made.speaker.say(text, urgent = true) },
                    notify = { notice, distance -> alerts.retarget(notice, distance, nav.intentFor(notice.target.at).intent, car.value) },
                )
            val router = NoticeRouter(core, settings, retargets)
            val runner = MissionRunner(core, made.service)
            return AppGraph(
                app = app,
                core = core,
                settings = settings,
                sensors = sensors,
                runner = runner,
                router = router,
                navigator = AppNavigator(core, runner, router),
                intake = PairLinkIntake(),
                discovery = discovery,
                demo = demo,
                nav = nav,
                speaker = made.speaker,
                clickTrack = made.clickTrack,
                clicks = ClickController(core, runner, settings, made.clickTrack, router),
                haptics = made.haptics,
                alerts = alerts,
                surveyTrail = SurveyTrailStore(core, runner),
                scope = scope,
                resumed = resumed,
                car = car,
            )
        }
    }
}

interface DemoSwitch {
    val active: Boolean

    fun demo(on: Boolean)
}
