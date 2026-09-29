package dev.newspicel.sdrmm.map

import java.util.Locale

object MapStyles {
    const val LIGHT = "https://tiles.openfreemap.org/styles/liberty"
    const val DARK = "https://tiles.openfreemap.org/styles/dark"
    private const val RGB_MASK = 0xFFFFFF

    fun blank(background: Int): String {
        val color = String.format(Locale.ROOT, "#%06X", background and RGB_MASK)
        return """{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"$color"}}]}"""
    }

    fun url(dark: Boolean): String = if (dark) DARK else LIGHT
}
