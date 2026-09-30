package dev.newspicel.sdrmm.ui.components

import android.content.res.Resources
import androidx.annotation.PluralsRes
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalResources

sealed interface UiText {
    data class Raw(
        val text: String,
    ) : UiText

    data class Res(
        @param:StringRes val id: Int,
        val args: List<Any> = emptyList(),
    ) : UiText

    data class Plural(
        @param:PluralsRes val id: Int,
        val count: Int,
    ) : UiText

    fun resolve(resources: Resources): String = when (this) {
        is Raw -> text
        is Res -> resources.getString(id, *args.map { if (it is UiText) it.resolve(resources) else it }.toTypedArray())
        is Plural -> resources.getQuantityString(id, count, count)
    }
}

@Composable
fun UiText.text(): String = resolve(LocalResources.current)
