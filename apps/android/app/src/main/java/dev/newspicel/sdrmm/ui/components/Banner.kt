package dev.newspicel.sdrmm.ui.components

import android.content.ClipData
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.NoticeLevel
import dev.newspicel.sdrmm.mission.Banner
import dev.newspicel.sdrmm.mission.NoticeRouter
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@Composable
fun BannerHost(
    router: NoticeRouter,
    modifier: Modifier = Modifier,
) {
    val banners by router.banners.collectAsStateWithLifecycle()
    val banner = banners.firstOrNull() as? Banner.Text ?: return
    var open by remember(banner.id) { mutableStateOf(false) }
    LaunchedEffect(banner.id, open) {
        if (open) return@LaunchedEffect
        delay(dismissAfterMs(banner.level))
        router.dismiss(banner)
    }
    Surface(
        color = levelColor(banner.level),
        contentColor = MaterialTheme.colorScheme.background,
        shape = MaterialTheme.shapes.medium,
        modifier =
        modifier
            .padding(horizontal = 12.dp, vertical = 8.dp)
            .fillMaxWidth()
            .clickable { open = true },
    ) {
        Text(banner.text.text(), modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp))
    }
    if (open) {
        BannerDetail(banner, onClose = {
            open = false
            router.dismiss(banner)
        })
    }
}

@Composable
private fun BannerDetail(
    banner: Banner.Text,
    onClose: () -> Unit,
) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    val text = banner.text.text()
    val full = listOfNotNull(text, banner.detail).joinToString("\n")
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(text) },
        text = banner.detail?.let { detail -> { Text(detail) } },
        confirmButton = {
            TextButton(onClick = {
                scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText(text, full))) }
            }) { Text(stringResource(R.string.copy)) }
        },
        dismissButton = { TextButton(onClick = onClose) { Text(stringResource(R.string.close)) } },
    )
}

@Composable
private fun levelColor(level: NoticeLevel): Color = when (level) {
    NoticeLevel.INFO -> MaterialTheme.colorScheme.primary
    NoticeLevel.WARN -> LocalStatusColors.current.warn
    NoticeLevel.ERROR -> LocalStatusColors.current.danger
}

fun dismissAfterMs(level: NoticeLevel): Long = when (level) {
    NoticeLevel.INFO -> 4_000L
    NoticeLevel.WARN -> 6_000L
    NoticeLevel.ERROR -> 8_000L
}
