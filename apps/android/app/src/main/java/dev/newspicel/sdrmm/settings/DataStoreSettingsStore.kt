package dev.newspicel.sdrmm.settings

import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.datastore.core.handlers.ReplaceFileCorruptionHandler
import androidx.datastore.preferences.core.MutablePreferences
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.doublePreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStoreFile
import dev.newspicel.sdrmm.ffi.HeadingMode
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.Mount
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import java.io.File
import java.io.IOException

class DataStoreSettingsStore(
    file: () -> File,
    private val defaultPhoneName: String,
    scope: CoroutineScope,
) : SettingsStore {
    private val failed = MutableStateFlow(false)
    private val unsaved = MutableStateFlow<String?>(null)
    private val store =
        PreferenceDataStoreFactory.create(
            corruptionHandler =
            ReplaceFileCorruptionHandler {
                failed.value = true
                emptyPreferences()
            },
            scope = scope,
            produceFile = file,
        )
    private val read =
        store.data
            .catch { error ->
                if (error !is IOException) throw error
                failed.value = true
                emit(emptyPreferences())
            }.map { decode(it) }

    override val readFailed: StateFlow<Boolean> = failed.asStateFlow()
    override val writeFailed: StateFlow<String?> = unsaved.asStateFlow()
    override val settings: StateFlow<AppSettings> =
        read.stateIn(scope, SharingStarted.Eagerly, AppSettings(phoneName = defaultPhoneName))

    override suspend fun current(): AppSettings = read.first()

    override suspend fun update(transform: (AppSettings) -> AppSettings) {
        try {
            store.edit { prefs -> encode(prefs, transform(decode(prefs))) }
            unsaved.value = null
        } catch (error: IOException) {
            unsaved.value = error.message ?: error.javaClass.simpleName
        }
    }

    private fun decode(prefs: Preferences): AppSettings {
        val lat = prefs[LAST_FIX_LAT]
        val lon = prefs[LAST_FIX_LON]
        return AppSettings(
            activeServerId = prefs[ACTIVE_SERVER_ID],
            phoneName = prefs[PHONE_NAME] ?: defaultPhoneName,
            headingMode = enumOr(prefs[HEADING_MODE], HeadingMode.AUTO),
            mount = enumOr(prefs[MOUNT], Mount.UPRIGHT),
            mountOffsetDeg = prefs[MOUNT_OFFSET_DEG] ?: 0.0,
            units = enumOr(prefs[UNITS], Units.Auto),
            navChoice = prefs[NAV_CHOICE]?.let { name -> NavChoice.entries.firstOrNull { it.name == name } },
            clicks = prefs[CLICKS] ?: true,
            haptics = prefs[HAPTICS] ?: true,
            voice = prefs[VOICE] ?: true,
            voiceName = prefs[VOICE_NAME],
            keepScreenOn = prefs[KEEP_SCREEN_ON] ?: true,
            mapTiles = prefs[MAP_TILES] ?: true,
            layers =
            MapLayers(
                rays = prefs[LAYER_RAYS] ?: true,
                heat = prefs[LAYER_HEAT] ?: true,
                ellipse = prefs[LAYER_ELLIPSE] ?: true,
            ),
            lastFix = if (lat != null && lon != null) LatLon(lat, lon) else null,
        )
    }

    private fun encode(
        prefs: MutablePreferences,
        value: AppSettings,
    ) {
        prefs.put(ACTIVE_SERVER_ID, value.activeServerId)
        prefs[PHONE_NAME] = value.phoneName
        prefs[HEADING_MODE] = value.headingMode.name
        prefs[MOUNT] = value.mount.name
        prefs[MOUNT_OFFSET_DEG] = value.mountOffsetDeg
        prefs[UNITS] = value.units.name
        prefs.put(NAV_CHOICE, value.navChoice?.name)
        prefs[CLICKS] = value.clicks
        prefs[HAPTICS] = value.haptics
        prefs[VOICE] = value.voice
        prefs.put(VOICE_NAME, value.voiceName)
        prefs[KEEP_SCREEN_ON] = value.keepScreenOn
        prefs[MAP_TILES] = value.mapTiles
        prefs[LAYER_RAYS] = value.layers.rays
        prefs[LAYER_HEAT] = value.layers.heat
        prefs[LAYER_ELLIPSE] = value.layers.ellipse
        prefs.put(LAST_FIX_LAT, value.lastFix?.lat)
        prefs.put(LAST_FIX_LON, value.lastFix?.lon)
    }

    private fun <T : Any> MutablePreferences.put(
        key: Preferences.Key<T>,
        value: T?,
    ) {
        if (value == null) remove(key) else set(key, value)
    }

    companion object {
        private val ACTIVE_SERVER_ID = stringPreferencesKey("active_server_id")
        private val PHONE_NAME = stringPreferencesKey("phone_name")
        private val HEADING_MODE = stringPreferencesKey("heading_mode")
        private val MOUNT = stringPreferencesKey("mount")
        private val MOUNT_OFFSET_DEG = doublePreferencesKey("mount_offset_deg")
        private val UNITS = stringPreferencesKey("units")
        private val NAV_CHOICE = stringPreferencesKey("nav_choice")
        private val CLICKS = booleanPreferencesKey("clicks")
        private val HAPTICS = booleanPreferencesKey("haptics")
        private val VOICE = booleanPreferencesKey("voice")
        private val VOICE_NAME = stringPreferencesKey("voice_name")
        private val KEEP_SCREEN_ON = booleanPreferencesKey("keep_screen_on")
        private val MAP_TILES = booleanPreferencesKey("map_tiles")
        private val LAYER_RAYS = booleanPreferencesKey("layer_rays")
        private val LAYER_HEAT = booleanPreferencesKey("layer_heat")
        private val LAYER_ELLIPSE = booleanPreferencesKey("layer_ellipse")
        private val LAST_FIX_LAT = doublePreferencesKey("last_fix_lat")
        private val LAST_FIX_LON = doublePreferencesKey("last_fix_lon")

        private inline fun <reified E : Enum<E>> enumOr(
            name: String?,
            fallback: E,
        ): E = enumValues<E>().firstOrNull { it.name == name } ?: fallback

        fun create(
            context: Context,
            scope: CoroutineScope,
        ): DataStoreSettingsStore = DataStoreSettingsStore(
            { context.preferencesDataStoreFile("settings") },
            defaultPhoneName(context),
            scope,
        )

        fun defaultPhoneName(context: Context): String = Settings.Global
            .getString(context.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() }
            ?: Build.MODEL
    }
}
