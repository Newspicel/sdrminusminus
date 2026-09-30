package dev.newspicel.sdrmm.map

import android.content.Context
import android.graphics.Bitmap
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.DrawableCompat
import androidx.core.graphics.drawable.toBitmap
import dev.newspicel.sdrmm.R

object MapIcons {
    const val YOU_ARROW = "you-arrow"
    private const val SIZE_DP = 28

    fun you(
        context: Context,
        color: Int,
    ): Bitmap? {
        val drawable = ContextCompat.getDrawable(context, R.drawable.ic_navigate)?.mutate() ?: return null
        DrawableCompat.setTint(drawable, color)
        val size = (SIZE_DP * context.resources.displayMetrics.density).toInt()
        return drawable.toBitmap(size, size)
    }
}
