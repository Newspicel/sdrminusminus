package dev.newspicel.sdrmm

import android.app.Application
import android.util.Log
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.ErrorText
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.core.UniffiCoreGateway
import dev.newspicel.sdrmm.demo.DemoGateway
import dev.newspicel.sdrmm.pair.Discovery
import dev.newspicel.sdrmm.pair.NsdDiscovery
import dev.newspicel.sdrmm.secrets.KeystoreCipher
import dev.newspicel.sdrmm.secrets.KeystoreVault
import dev.newspicel.sdrmm.settings.DataStoreSettingsStore
import dev.newspicel.sdrmm.settings.MemorySettingsStore
import dev.newspicel.sdrmm.settings.SettingsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

open class SdrmmApp :
    Application(),
    DemoSwitch {
    private val state = MutableStateFlow<Startup?>(null)
    val startup: StateFlow<Startup?> = state.asStateFlow()
    protected val appScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var inDemo = false
    private val realCore: CoreLoad by lazy { loadCore() }
    private val realSettings: SettingsStore by lazy { createSettings() }

    override val active: Boolean get() = inDemo

    override fun onCreate() {
        super.onCreate()
        show(build(demo = false))
    }

    override fun demo(on: Boolean) {
        if (on == inDemo) return
        (state.value as? Startup.Ready)?.graph?.stop()
        inDemo = on
        show(build(on))
    }

    private fun show(startup: Startup) {
        state.value = startup
        (startup as? Startup.Ready)?.graph?.start()
    }

    protected open fun build(demo: Boolean): Startup {
        if (demo) {
            val settings = MemorySettingsStore(realSettings.settings.value.copy(activeServerId = DemoGateway.SERVER_ID))
            val gateway = DemoGateway((realCore as? CoreLoad.Loaded)?.core)
            return Startup.Ready(AppGraph.assemble(this, gateway, settings, this, createDiscovery()))
        }
        return when (val core = realCore) {
            is CoreLoad.Broken -> Startup.Failed(core.message)
            is CoreLoad.Loaded -> Startup.Ready(AppGraph.assemble(this, core.core, realSettings, this, createDiscovery()))
        }
    }

    protected open fun createDiscovery(): Discovery = NsdDiscovery(this)

    protected open fun createSettings(): SettingsStore = DataStoreSettingsStore.create(this, appScope)

    protected open fun createCore(): Outcome<CoreGateway> = UniffiCoreGateway.create(this, KeystoreVault(File(noBackupFilesDir, "vault"), KeystoreCipher()))

    private fun loadCore(): CoreLoad = try {
        when (val created = createCore()) {
            is Outcome.Ok -> CoreLoad.Loaded(created.value)
            is Outcome.Failed -> CoreLoad.Broken(ErrorText.detail(created.error))
        }
    } catch (error: LinkageError) {
        Log.e(TAG, "Core library failed to load", error)
        CoreLoad.Broken(error.message ?: error.javaClass.simpleName)
    }

    private sealed interface CoreLoad {
        class Loaded(
            val core: CoreGateway,
        ) : CoreLoad

        class Broken(
            val message: String,
        ) : CoreLoad
    }

    private companion object {
        const val TAG = "SdrmmApp"
    }
}
