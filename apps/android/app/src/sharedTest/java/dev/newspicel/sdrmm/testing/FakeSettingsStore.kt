package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.settings.AppSettings
import dev.newspicel.sdrmm.settings.SettingsStore
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update

class FakeSettingsStore(
    initial: AppSettings = AppSettings(phoneName = "Pixel Test"),
) : SettingsStore {
    override val settings = MutableStateFlow(initial)
    override val readFailed = MutableStateFlow(false)
    override val writeFailed = MutableStateFlow<String?>(null)

    override suspend fun current(): AppSettings = settings.value

    override suspend fun update(transform: (AppSettings) -> AppSettings) {
        settings.update(transform)
    }
}
