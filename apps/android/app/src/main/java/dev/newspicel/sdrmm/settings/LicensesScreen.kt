package dev.newspicel.sdrmm.settings

import android.content.Context
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.LicenseEntry
import dev.newspicel.sdrmm.ui.components.SectionTitle
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import java.io.IOException

sealed interface AndroidNotices {
    data class Text(
        val paragraphs: List<String>,
    ) : AndroidNotices

    data class Unreadable(
        val reason: String,
    ) : AndroidNotices

    companion object {
        const val ASSET = "NOTICES.txt"

        fun read(context: Context): AndroidNotices = try {
            val text = context.assets.open(ASSET).bufferedReader(Charsets.UTF_8).use { it.readText() }
            Text(text.split("\n\n").filter { it.isNotBlank() })
        } catch (error: IOException) {
            Unreadable(error.message ?: ASSET)
        }
    }
}

@Composable
fun LicensesScreen(graph: AppGraph) {
    val context = LocalContext.current
    val entries = remember(graph) { graph.core.licenses() }
    val notices = remember(context) { AndroidNotices.read(context) }
    LicensesContent(entries, notices, onBack = graph.navigator::back)
}

@Composable
fun LicensesContent(
    entries: List<LicenseEntry>,
    notices: AndroidNotices,
    onBack: () -> Unit,
) {
    var open by rememberSaveable { mutableStateOf<String?>(null) }
    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        TopBar(stringResource(R.string.licenses), onBack = onBack)
        LazyColumn(Modifier.fillMaxSize()) {
            items(entries, key = { "${it.name}@${it.version}" }) { entry ->
                val id = "${entry.name}@${entry.version}"
                ListItem(
                    headlineContent = { Text(listOfNotNull(entry.name, entry.version).joinToString(" ")) },
                    supportingContent = { Text(entry.license) },
                    colors = ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.background),
                    modifier = Modifier.clickable { open = if (open == id) null else id },
                )
                if (open == id) Mono(entry.text)
            }
            item { SectionTitle(stringResource(R.string.licenses_android)) }
            when (notices) {
                is AndroidNotices.Text -> items(notices.paragraphs) { Mono(it) }

                is AndroidNotices.Unreadable ->
                    item {
                        Text(
                            stringResource(R.string.licenses_unreadable, notices.reason),
                            color = LocalStatusColors.current.danger,
                            modifier = Modifier.padding(16.dp),
                        )
                    }
            }
        }
    }
}

@Composable
private fun Mono(text: String) {
    Text(
        text,
        fontFamily = FontFamily.Monospace,
        style = MaterialTheme.typography.bodySmall,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
    )
}
