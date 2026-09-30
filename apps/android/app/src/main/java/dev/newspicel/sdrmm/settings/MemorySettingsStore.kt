package dev.newspicel.sdrmm.settings

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

class MemorySettingsStore(
    initial: AppSettings,
) : SettingsStore {
    private val state = MutableStateFlow(initial)
    override val settings: StateFlow<AppSettings> = state.asStateFlow()
    override val readFailed: StateFlow<Boolean> = MutableStateFlow(false).asStateFlow()
    override val writeFailed: StateFlow<String?> = MutableStateFlow<String?>(null).asStateFlow()

    override suspend fun current(): AppSettings = state.value

    override suspend fun update(transform: (AppSettings) -> AppSettings) {
        state.update(transform)
    }
}
