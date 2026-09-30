package dev.newspicel.sdrmm.testing

import android.app.Application
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.DemoSwitch
import dev.newspicel.sdrmm.OutputFactory
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.pair.Discovery
import dev.newspicel.sdrmm.settings.SettingsStore

object TestAppGraph {
    fun create(
        app: Application,
        core: CoreGateway = FakeCoreGateway(),
        settings: SettingsStore = FakeSettingsStore(),
        discovery: Discovery = FakeDiscovery(),
        demo: DemoSwitch = NoDemo(),
        outputs: OutputFactory = FakeOutputs(),
    ): AppGraph = AppGraph.assemble(app, core, settings, demo, discovery, outputs)
}

class NoDemo : DemoSwitch {
    val requests = mutableListOf<Boolean>()
    override val active: Boolean = false

    override fun demo(on: Boolean) {
        requests += on
    }
}
