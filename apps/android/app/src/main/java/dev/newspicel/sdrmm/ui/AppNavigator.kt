package dev.newspicel.sdrmm.ui

import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.snapshots.SnapshotStateList
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.mission.MissionRunner
import dev.newspicel.sdrmm.mission.NoticeRouter
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

class AppNavigator(
    private val core: CoreGateway,
    private val runner: MissionRunner,
    private val router: NoticeRouter,
) {
    val backStack: SnapshotStateList<Destination> = mutableStateListOf(start())

    private fun start(): Destination = when (val saved = core.savedServers()) {
        is Outcome.Ok -> {
            if (saved.value.isEmpty()) Destination.Pair else Destination.Missions
        }

        is Outcome.Failed -> {
            router.report(saved.error)
            Destination.Pair
        }
    }

    fun top(): Destination? = backStack.lastOrNull()

    fun push(destination: Destination) {
        if (top() != destination) backStack.add(destination)
    }

    fun back() {
        if (backStack.size <= 1) return
        val left = backStack.removeAt(backStack.lastIndex)
        if (left is Destination.Mission) runner.close()
    }

    fun openMission(mission: Mission) {
        when (val opened = runner.open(mission.id)) {
            is Outcome.Ok -> push(Destination.Mission(mission.id, mission.kind))
            is Outcome.Failed -> router.report(opened.error)
        }
    }

    fun paired() {
        replace(Destination.Missions)
    }

    fun toPair() {
        replace(Destination.Pair)
    }

    fun showPair() {
        if (top() !is Destination.Pair) push(Destination.Pair)
    }

    fun run(
        scope: CoroutineScope,
        main: CoroutineDispatcher = Dispatchers.Main.immediate,
    ): Job = scope.launch(main) { runner.open.collect(::follow) }

    private fun follow(id: String?) {
        if (id == null) return
        val top = top()
        if (top is Destination.Mission && top.id == id) return
        val kind =
            core.missions.value
                ?.missions
                ?.firstOrNull { it.id == id }
                ?.kind ?: return
        val shown = Destination.Mission(id, kind)
        if (top is Destination.Mission) backStack[backStack.lastIndex] = shown else backStack.add(shown)
    }

    private fun replace(destination: Destination) {
        if (backStack.any { it is Destination.Mission }) runner.close()
        backStack.clear()
        backStack.add(destination)
    }
}
