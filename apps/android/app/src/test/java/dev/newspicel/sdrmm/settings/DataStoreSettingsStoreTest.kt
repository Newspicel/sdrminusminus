package dev.newspicel.sdrmm.settings

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.HeadingMode
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.Mount
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assume.assumeFalse
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class DataStoreSettingsStoreTest {
    @get:Rule val folder = TemporaryFolder()

    private fun <T> withStore(
        file: File,
        block: suspend (DataStoreSettingsStore) -> T,
    ): T = runBlocking {
        val job = SupervisorJob()
        val scope = CoroutineScope(Dispatchers.IO + job)
        try {
            block(DataStoreSettingsStore({ file }, "Pixel", scope))
        } finally {
            job.cancelAndJoin()
        }
    }

    @Test
    fun defaults_update() {
        val file = File(folder.root, "settings.preferences_pb")
        withStore(file) { store ->
            val defaults = store.current()
            assertThat(defaults).isEqualTo(AppSettings(phoneName = "Pixel"))
            store.update {
                it.copy(
                    activeServerId = "s1",
                    headingMode = HeadingMode.COURSE,
                    mount = Mount.FLAT,
                    mountOffsetDeg = -4.0,
                    units = Units.Imperial,
                    navChoice = NavChoice.Ask,
                    voice = false,
                    layers = MapLayers(heat = false),
                    lastFix = LatLon(52.5, 13.4),
                )
            }
            assertThat(store.settings.first { it.activeServerId == "s1" }.mount).isEqualTo(Mount.FLAT)
        }
        withStore(file) { store ->
            val loaded = store.current()
            assertThat(loaded.activeServerId).isEqualTo("s1")
            assertThat(loaded.headingMode).isEqualTo(HeadingMode.COURSE)
            assertThat(loaded.mountOffsetDeg).isEqualTo(-4.0)
            assertThat(loaded.units).isEqualTo(Units.Imperial)
            assertThat(loaded.navChoice).isEqualTo(NavChoice.Ask)
            assertThat(loaded.voice).isFalse()
            assertThat(loaded.layers).isEqualTo(MapLayers(heat = false))
            assertThat(loaded.lastFix).isEqualTo(LatLon(52.5, 13.4))
            store.update { it.copy(activeServerId = null, navChoice = null, lastFix = null) }
            assertThat(store.current().activeServerId).isNull()
            assertThat(store.current().lastFix).isNull()
            assertThat(store.readFailed.value).isFalse()
        }
    }

    @Test
    fun a_corrupt_file_reads_defaults_says_so_and_saves_again() {
        val file = File(folder.root, "settings.preferences_pb")
        file.writeBytes(byteArrayOf(0x7f, 0x13, 0x00, 0x42))
        withStore(file) { store ->
            assertThat(store.current()).isEqualTo(AppSettings(phoneName = "Pixel"))
            assertThat(store.readFailed.first { it }).isTrue()
            store.update { it.copy(activeServerId = "s2") }
            assertThat(store.writeFailed.value).isNull()
        }
        withStore(file) { store -> assertThat(store.current().activeServerId).isEqualTo("s2") }
    }

    @Test
    fun a_failed_write_is_reported_not_thrown() {
        val dir = folder.newFolder("locked")
        val file = File(dir, "settings.preferences_pb")
        withStore(file) { store ->
            store.update { it.copy(activeServerId = "s1") }
            dir.setWritable(false)
            try {
                assumeFalse(dir.canWrite())
                store.update { it.copy(activeServerId = "s2") }
                assertThat(store.writeFailed.value).isNotNull()
                assertThat(store.current().activeServerId).isEqualTo("s1")
                dir.setWritable(true)
                store.update { it.copy(activeServerId = "s3") }
                assertThat(store.writeFailed.value).isNull()
            } finally {
                dir.setWritable(true)
            }
        }
    }
}
