package dev.newspicel.sdrmm.settings

import com.google.common.truth.Truth.assertThat
import org.junit.Test
import java.io.File

class NoticesTest {
    @Test
    fun names_every_bundled_component() {
        val text = File("src/main/assets/${AndroidNotices.ASSET}").readText()
        for (name in listOf(
            "Jetpack Compose",
            "AndroidX",
            "CameraX",
            "ZXing",
            "MapLibre Native",
            "Car App Library",
            "JNA",
            "kotlinx-coroutines",
            "OpenFreeMap",
            "OpenMapTiles",
            "OpenStreetMap",
            "Material Symbols",
        )) {
            assertThat(text).contains(name)
        }
        assertThat(text).contains("Apache License")
        assertThat(text).doesNotContain("\u2014")
    }
}
