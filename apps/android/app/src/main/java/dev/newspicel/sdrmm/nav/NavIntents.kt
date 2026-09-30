package dev.newspicel.sdrmm.nav

import android.content.Intent
import androidx.car.app.CarContext
import androidx.core.net.toUri

object NavIntents {
    const val MAPS_PACKAGE = "com.google.android.apps.maps"

    fun googleMaps(uri: String): Intent = view(uri).setPackage(MAPS_PACKAGE)

    fun chooser(
        uri: String,
        title: String,
    ): Intent = Intent.createChooser(view(uri), title)

    fun car(uri: String): Intent = Intent(CarContext.ACTION_NAVIGATE, uri.toUri())

    fun view(uri: String): Intent = Intent(Intent.ACTION_VIEW, uri.toUri())
}
