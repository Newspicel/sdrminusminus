package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.pair.Discovery
import kotlinx.coroutines.flow.MutableStateFlow

class FakeDiscovery : Discovery {
    override val found = MutableStateFlow<List<DiscoveredServer>>(emptyList())
    override val failed = MutableStateFlow(false)
    var running = 0

    override fun start() {
        running += 1
    }

    override fun stop() {
        running -= 1
    }
}
