package dev.newspicel.sdrmm.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner
import androidx.lifecycle.viewmodel.navigation3.rememberViewModelStoreNavEntryDecorator
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.navigation3.ui.NavDisplay
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.SdrmmApp
import dev.newspicel.sdrmm.Startup
import dev.newspicel.sdrmm.pair.PairScreen
import dev.newspicel.sdrmm.settings.LicensesScreen
import dev.newspicel.sdrmm.settings.SettingsScreen
import dev.newspicel.sdrmm.ui.components.BannerHost

@Composable
fun AppRoot(app: SdrmmApp) {
    val startup by app.startup.collectAsStateWithLifecycle()
    Surface(color = MaterialTheme.colorScheme.background, modifier = Modifier.fillMaxSize()) {
        when (val current = startup) {
            is Startup.Ready -> key(current.graph) { GraphRoot(current.graph) }
            is Startup.Failed -> CoreFailed(current.message)
            null -> Unit
        }
    }
}

@Composable
fun GraphRoot(graph: AppGraph) {
    val owner = remember(graph) { GraphViewModels(graph.viewModels) }
    CompositionLocalProvider(LocalViewModelStoreOwner provides owner) { GraphContent(graph) }
}

private class GraphViewModels(
    override val viewModelStore: ViewModelStore,
) : ViewModelStoreOwner

@Composable
private fun GraphContent(graph: AppGraph) {
    Box(Modifier.fillMaxSize()) {
        NavDisplay(
            backStack = graph.navigator.backStack,
            onBack = graph.navigator::back,
            entryDecorators =
            listOf(
                rememberSaveableStateHolderNavEntryDecorator(),
                rememberViewModelStoreNavEntryDecorator(),
            ),
            entryProvider =
            entryProvider {
                entry<Destination.Pair> { PairScreen(graph) }
                entry<Destination.Missions> { PendingScreen(graph, stringResource(R.string.missions_title)) }
                entry<Destination.Mission> { mission -> PendingScreen(graph, missionTitle(graph, mission.id)) }
                entry<Destination.Settings> { SettingsScreen(graph) }
                entry<Destination.Licenses> { LicensesScreen(graph) }
            },
        )
        Column(Modifier.align(Alignment.BottomCenter).safeDrawingPadding()) { DemoStrip(graph) }
        Column(Modifier.align(Alignment.TopCenter).safeDrawingPadding()) { BannerHost(graph.router) }
    }
}

@Composable
private fun CoreFailed(message: String) {
    Column(Modifier.safeDrawingPadding().padding(24.dp)) {
        Text(stringResource(R.string.core_failed), style = MaterialTheme.typography.headlineSmall)
        Text(message, modifier = Modifier.padding(top = 8.dp))
    }
}
