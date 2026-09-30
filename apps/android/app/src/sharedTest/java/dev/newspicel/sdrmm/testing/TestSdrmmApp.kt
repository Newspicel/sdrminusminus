package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.OutputFactory
import dev.newspicel.sdrmm.SdrmmApp
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.mission.MissionServiceControl
import dev.newspicel.sdrmm.pair.Discovery
import dev.newspicel.sdrmm.settings.SettingsStore

class TestSdrmmApp : SdrmmApp() {
    val fakeCore = FakeCoreGateway()
    val fakeSettings = FakeSettingsStore()
    val fakeDiscovery = FakeDiscovery()
    val fakeOutputs = FakeOutputs(service = MissionServiceControl(this))

    override fun createCore(): Outcome<CoreGateway> = Outcome.Ok(fakeCore)

    override fun createSettings(): SettingsStore = fakeSettings

    override fun createDiscovery(): Discovery = fakeDiscovery

    override fun createOutputs(): OutputFactory = fakeOutputs
}
