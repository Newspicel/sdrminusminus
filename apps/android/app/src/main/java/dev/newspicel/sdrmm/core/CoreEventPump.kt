package dev.newspicel.sdrmm.core

import dev.newspicel.sdrmm.ffi.MobileCore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

class CoreEventPump(
    private val core: MobileCore,
    private val hub: CoreStateHub,
) {
    fun run(scope: CoroutineScope): Job = scope.launch(Dispatchers.Default) {
        while (true) {
            val event = core.nextEvent() ?: break
            hub.dispatch(event)
        }
    }
}
