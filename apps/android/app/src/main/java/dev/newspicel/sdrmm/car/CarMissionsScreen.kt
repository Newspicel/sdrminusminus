package dev.newspicel.sdrmm.car

import androidx.car.app.CarContext
import androidx.car.app.CarToast
import androidx.car.app.Screen
import androidx.car.app.model.Action
import androidx.car.app.model.Header
import androidx.car.app.model.ItemList
import androidx.car.app.model.ListTemplate
import androidx.car.app.model.Row
import androidx.car.app.model.Template
import androidx.car.app.navigation.model.MapController
import androidx.car.app.navigation.model.MapWithContentTemplate
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.ErrorText
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.Mission
import dev.newspicel.sdrmm.ffi.MissionKind
import dev.newspicel.sdrmm.mission.BackgroundState
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch

class CarMissionsScreen(
    carContext: CarContext,
    private val graph: AppGraph,
    private val renderer: CarMapRenderer,
) : Screen(carContext) {
    init {
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                combine(graph.core.link, graph.core.missions) { link, missions -> listing(link) to missions?.missions?.filter { it.kind == MissionKind.DF_DRIVE } }
                    .distinctUntilChanged()
                    .collect { invalidate() }
            }
        }
    }

    override fun onGetTemplate(): Template {
        val header =
            Header
                .Builder()
                .setTitle(carContext.getString(R.string.missions_title))
                .setStartHeaderAction(Action.APP_ICON)
                .build()
        val list = ListTemplate.Builder().setHeader(header)
        val link = graph.core.link.value
        if (link is LinkState.Connecting) list.setLoading(true) else list.setSingleList(items(link))
        return MapWithContentTemplate
            .Builder()
            .setContentTemplate(list.build())
            .setMapController(
                MapController
                    .Builder()
                    .setMapActionStrip(CarActions.mapStrip(carContext, renderer))
                    .setPanModeListener { panning -> if (panning) renderer.free() }
                    .build(),
            )
            .build()
    }

    private fun items(link: LinkState): ItemList {
        val items = ItemList.Builder()
        if (link !is LinkState.Online) {
            items.addItem(Row.Builder().setTitle(carContext.getString(R.string.link_offline)).build())
            return items.build()
        }
        val missions = graph.core.missions.value?.missions.orEmpty().filter { it.kind == MissionKind.DF_DRIVE }
        if (missions.isEmpty()) {
            items.addItem(Row.Builder().setTitle(carContext.getString(R.string.car_no_df)).build())
            return items.build()
        }
        missions.forEach { items.addItem(row(it)) }
        return items.build()
    }

    private fun row(mission: Mission): Row {
        val row = Row.Builder().setTitle(mission.title)
        if (!mission.ready) {
            row.addText(mission.blocker ?: mission.detail)
            return row.build()
        }
        row.addText(mission.detail).setBrowsable(true).setOnClickListener { open(mission) }
        return row.build()
    }

    private fun open(mission: Mission) {
        when (val opened = graph.runner.open(mission.id)) {
            is Outcome.Ok -> {
                if (graph.runner.background.value is BackgroundState.Off) toast(carContext.getString(R.string.chip_background_off))
                screenManager.push(CarDfScreen(carContext, graph, renderer, mission.id))
            }

            is Outcome.Failed -> {
                toast(ErrorText.short(opened.error).resolve(carContext.resources))
            }
        }
    }

    private fun toast(text: String) {
        CarToast.makeText(carContext, text, CarToast.LENGTH_LONG).show()
    }

    private fun listing(link: LinkState): Int = when (link) {
        is LinkState.Online -> 0
        is LinkState.Connecting -> 1
        else -> 2
    }
}
