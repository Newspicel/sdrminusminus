package dev.newspicel.sdrmm.car

import androidx.car.app.CarContext
import androidx.car.app.CarToast
import androidx.car.app.HostException
import androidx.car.app.Screen
import androidx.car.app.model.Action
import androidx.car.app.model.ActionStrip
import androidx.car.app.model.CarIcon
import androidx.car.app.model.Template
import androidx.car.app.navigation.model.MessageInfo
import androidx.car.app.navigation.model.NavigationTemplate
import androidx.core.graphics.drawable.IconCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.ErrorText
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.df.RoseGeometry
import dev.newspicel.sdrmm.df.RoseModel
import dev.newspicel.sdrmm.ffi.DfView
import dev.newspicel.sdrmm.ffi.MissionCommand
import dev.newspicel.sdrmm.ffi.MissionControl
import dev.newspicel.sdrmm.ffi.NavApp
import dev.newspicel.sdrmm.nav.NavIntents
import dev.newspicel.sdrmm.ui.Format
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.sample
import kotlinx.coroutines.launch

class CarDfScreen(
    carContext: CarContext,
    private val graph: AppGraph,
    private val renderer: CarMapRenderer,
    private val missionId: String,
) : Screen(carContext) {
    private var panel = panel()
    private var rose = rose()

    init {
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                launch { refresh() }
                launch { overlay() }
            }
        }
        lifecycleScope.launch {
            graph.runner.open.first { it != missionId }
            screenManager.pop()
        }
        lifecycle.addObserver(
            object : DefaultLifecycleObserver {
                override fun onDestroy(owner: LifecycleOwner) {
                    renderer.scene.value?.showDf(null, graph.settings.settings.value.layers)
                }
            },
        )
    }

    @OptIn(FlowPreview::class)
    private suspend fun refresh() {
        combine(graph.core.df, graph.core.pose, graph.sensors.lastFix, graph.settings.settings, graph.core.missions) { _, _, _, _, _ -> panel() to rose() }
            .sample(PANEL_MS)
            .collect { (next, model) ->
                if (next == panel) return@collect
                panel = next
                rose = model
                invalidate()
            }
    }

    @OptIn(FlowPreview::class)
    private suspend fun overlay() {
        combine(renderer.scene, graph.core.df.map { view() }, graph.settings.settings.map { it.layers }) { scene, view, layers -> Triple(scene, view, layers) }
            .sample(OVERLAY_MS)
            .collect { (scene, view, layers) -> scene?.showDf(view, layers) }
    }

    private fun view(): DfView? = graph.core.df.value?.takeIf { it.mission == missionId }

    private fun controls(): List<MissionControl> = graph.core.missions.value
        ?.missions
        ?.firstOrNull { it.id == missionId }
        ?.controls
        .orEmpty()

    private fun panel(): CarPanel = CarPanel.make(
        view(),
        graph.core.pose.value,
        graph.sensors.lastFix.value,
        controls(),
        Format.resolve(graph.settings.settings.value.units),
        graph.core,
        carContext.resources,
    )

    private fun rose(): RoseModel = RoseGeometry.model(view(), graph.core.pose.value)

    override fun onGetTemplate(): Template {
        val image = RoseBitmap.draw(rose, ROSE_PX, carContext.isDarkMode)
        val info =
            MessageInfo
                .Builder(panel.title)
                .setText(panel.text)
                .setImage(CarIcon.Builder(IconCompat.createWithBitmap(image)).build())
                .build()
        return NavigationTemplate
            .Builder()
            .setNavigationInfo(info)
            .setActionStrip(actions())
            .setMapActionStrip(CarActions.mapStrip(carContext, renderer))
            .setPanModeListener { panning -> if (panning) renderer.free() }
            .build()
    }

    private fun actions(): ActionStrip {
        val strip =
            ActionStrip.Builder().addAction(
                Action
                    .Builder()
                    .setTitle(carContext.getString(R.string.navigate))
                    .setIcon(CarActions.icon(carContext, R.drawable.ic_navigate))
                    .setEnabled(panel.canNavigate)
                    .setOnClickListener(::navigate)
                    .build(),
            )
        if (panel.canCalibrate) strip.addAction(Action.Builder().setTitle(carContext.getString(R.string.calibrate)).setOnClickListener(::calibrate).build())
        if (panel.canClear) strip.addAction(Action.Builder().setTitle(carContext.getString(R.string.clear)).setOnClickListener(::clear).build())
        return strip.build()
    }

    private fun navigate() {
        val target = view()?.target ?: return
        try {
            carContext.startCarApp(NavIntents.car(graph.core.navUri(target.at, NavApp.CAR)))
            graph.nav.handedOff(missionId)
        } catch (_: HostException) {
            toast(carContext.getString(R.string.no_map_app))
        } catch (_: SecurityException) {
            toast(carContext.getString(R.string.no_map_app))
        }
    }

    private fun calibrate() = send(MissionCommand.Calibrate, null)

    private fun clear() = send(MissionCommand.ClearFusion, R.string.fusion_cleared)

    private fun send(
        command: MissionCommand,
        done: Int?,
    ) {
        lifecycleScope.launch {
            when (val sent = graph.core.send(command)) {
                is Outcome.Ok -> done?.let { toast(carContext.getString(it)) }
                is Outcome.Failed -> toast(ErrorText.short(sent.error).resolve(carContext.resources))
            }
        }
    }

    private fun toast(text: String) {
        CarToast.makeText(carContext, text, CarToast.LENGTH_LONG).show()
    }

    private companion object {
        const val ROSE_PX = 256
        const val PANEL_MS = 1_000L
        const val OVERLAY_MS = 500L
    }
}
