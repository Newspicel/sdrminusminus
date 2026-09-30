package dev.newspicel.sdrmm.pair

import androidx.camera.compose.CameraXViewfinder
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.newspicel.sdrmm.R

@Composable
fun ScannerSheet(
    onPayload: (String) -> Unit,
    onClose: () -> Unit,
) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val scanner = remember { QrScanner(context) }
    val request by scanner.surfaceRequests.collectAsStateWithLifecycle()
    val failure by scanner.failure.collectAsStateWithLifecycle()
    LaunchedEffect(scanner) { scanner.bind(owner) }
    LaunchedEffect(scanner) { scanner.results.collect { if (it is QrResult.Payload) onPayload(it.text) } }
    DisposableEffect(scanner) { onDispose { scanner.unbind() } }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            request?.let { CameraXViewfinder(surfaceRequest = it, modifier = Modifier.fillMaxSize()) }
            Box(
                Modifier
                    .align(Alignment.Center)
                    .fillMaxWidth(FRAME_FRACTION)
                    .aspectRatio(1f)
                    .border(2.dp, MaterialTheme.colorScheme.primary, MaterialTheme.shapes.medium),
            )
            failure?.let { detail ->
                Text(
                    stringResource(R.string.camera_off),
                    color = Color.White,
                    modifier = Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(24.dp),
                )
                Text(detail, color = Color.White, modifier = Modifier.align(Alignment.Center).padding(24.dp))
            }
            IconButton(onClick = onClose, modifier = Modifier.align(Alignment.TopEnd).safeDrawingPadding()) {
                Icon(painterResource(R.drawable.ic_close), contentDescription = stringResource(R.string.close), tint = Color.White)
            }
        }
    }
}

private const val FRAME_FRACTION = 0.7f
