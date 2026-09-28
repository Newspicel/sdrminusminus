package dev.newspicel.sdrmm.ui

import dev.newspicel.sdrmm.ffi.MissionKind

sealed interface Destination {
    data object Pair : Destination

    data object Missions : Destination

    data class Mission(
        val id: String,
        val kind: MissionKind,
    ) : Destination

    data object Settings : Destination

    data object Licenses : Destination
}
