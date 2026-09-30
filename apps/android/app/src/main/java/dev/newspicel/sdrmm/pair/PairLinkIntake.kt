package dev.newspicel.sdrmm.pair

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.getAndUpdate

class PairLinkIntake {
    private val pending = MutableStateFlow<String?>(null)
    val link: StateFlow<String?> = pending.asStateFlow()

    fun offer(uri: String) {
        pending.value = uri
    }

    fun take(): String? = pending.getAndUpdate { null }

    companion object {
        const val PREFIX = "sdrmm://pair"

        fun isPairLink(text: String): Boolean = text.startsWith(PREFIX)
    }
}
