package dev.newspicel.sdrmm.settings

import kotlinx.coroutines.flow.StateFlow

interface SettingsStore {
    val settings: StateFlow<AppSettings>
    val readFailed: StateFlow<Boolean>
    val writeFailed: StateFlow<String?>

    suspend fun current(): AppSettings

    suspend fun update(transform: (AppSettings) -> AppSettings)
}
