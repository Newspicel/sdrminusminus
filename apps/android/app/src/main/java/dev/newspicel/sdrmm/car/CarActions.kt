package dev.newspicel.sdrmm.car

import androidx.annotation.DrawableRes
import androidx.car.app.CarContext
import androidx.car.app.model.Action
import androidx.car.app.model.ActionStrip
import androidx.car.app.model.CarIcon
import androidx.core.graphics.drawable.IconCompat
import dev.newspicel.sdrmm.R

object CarActions {
    fun icon(
        carContext: CarContext,
        @DrawableRes id: Int,
    ): CarIcon = CarIcon.Builder(IconCompat.createWithResource(carContext, id)).build()

    fun mapStrip(
        carContext: CarContext,
        renderer: CarMapRenderer,
    ): ActionStrip = ActionStrip
        .Builder()
        .addAction(Action.PAN)
        .addAction(iconAction(carContext, R.drawable.ic_zoom_in) { renderer.zoom(1.0) })
        .addAction(iconAction(carContext, R.drawable.ic_zoom_out) { renderer.zoom(-1.0) })
        .addAction(iconAction(carContext, R.drawable.ic_follow, renderer::recentre))
        .build()

    private fun iconAction(
        carContext: CarContext,
        @DrawableRes id: Int,
        onClick: () -> Unit,
    ): Action = Action
        .Builder()
        .setIcon(icon(carContext, id))
        .setOnClickListener(onClick)
        .build()
}
