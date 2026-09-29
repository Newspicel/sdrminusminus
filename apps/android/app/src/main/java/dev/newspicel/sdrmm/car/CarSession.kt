package dev.newspicel.sdrmm.car

import android.content.Intent
import android.content.res.Configuration
import androidx.annotation.StringRes
import androidx.car.app.AppManager
import androidx.car.app.CarContext
import androidx.car.app.Screen
import androidx.car.app.Session
import androidx.car.app.model.Action
import androidx.car.app.model.Header
import androidx.car.app.model.MessageTemplate
import androidx.car.app.model.Template
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.lifecycleScope
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.Startup
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.sensors.Holder

class CarSession(
    private val startup: Startup?,
) : Session() {
    private var renderer: CarMapRenderer? = null

    override fun onCreateScreen(intent: Intent): Screen {
        val graph =
            when (startup) {
                is Startup.Failed -> return MessageScreen(carContext, R.string.core_failed, startup.message)
                null -> return MessageScreen(carContext, R.string.core_failed, null)
                is Startup.Ready -> startup.graph
            }
        val saved = graph.core.savedServers()
        if (saved !is Outcome.Ok || saved.value.isEmpty()) return MessageScreen(carContext, R.string.car_pair_on_phone, null)
        val map = CarMapRenderer(carContext, graph, lifecycleScope, carContext.isDarkMode)
        renderer = map
        carContext.getCarService(AppManager::class.java).setSurfaceCallback(map)
        lifecycle.addObserver(map)
        lifecycle.addObserver(CarHold(graph))
        return CarMissionsScreen(carContext, graph, map)
    }

    override fun onCarConfigurationChanged(newConfiguration: Configuration) {
        renderer?.restyle(carContext.isDarkMode)
    }

    private class CarHold(
        private val graph: AppGraph,
    ) : DefaultLifecycleObserver {
        override fun onStart(owner: LifecycleOwner) {
            graph.carShowing(true)
            graph.sensors.acquire(Holder.Car)
        }

        override fun onStop(owner: LifecycleOwner) {
            graph.sensors.release(Holder.Car)
            graph.carShowing(false)
        }
    }
}

class MessageScreen(
    carContext: CarContext,
    @param:StringRes private val title: Int,
    private val detail: String?,
) : Screen(carContext) {
    override fun onGetTemplate(): Template {
        val header =
            Header
                .Builder()
                .setTitle(carContext.getString(R.string.app_name))
                .setStartHeaderAction(Action.APP_ICON)
                .build()
        val message = MessageTemplate.Builder(carContext.getString(title)).setHeader(header)
        detail?.let(message::setDebugMessage)
        return message.build()
    }
}
