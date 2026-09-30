package dev.newspicel.sdrmm.pair

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.newspicel.sdrmm.AppGraph
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.permissions.Need
import dev.newspicel.sdrmm.permissions.PermissionGate
import dev.newspicel.sdrmm.permissions.rememberPermissionGate
import dev.newspicel.sdrmm.ui.components.BOTTOM_ROOM
import dev.newspicel.sdrmm.ui.components.SectionTitle
import dev.newspicel.sdrmm.ui.components.TopBar
import dev.newspicel.sdrmm.ui.components.text
import dev.newspicel.sdrmm.ui.theme.LocalStatusColors

@Composable
fun PairScreen(graph: AppGraph) {
    val model =
        viewModel {
            PairViewModel(graph.core, graph.settings, graph.discovery, graph.intake, graph.navigator, graph::activate)
        }
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val gate =
        rememberPermissionGate { need, allowed ->
            when (need) {
                Need.Camera -> if (allowed) model.scan() else model.setCameraAllowed(false)

                Need.LocalNetwork -> {
                    model.setLocalNetworkAllowed(allowed)
                    graph.permissionsChanged()
                    model.appear()
                }

                else -> graph.permissionsChanged()
            }
        }
    LaunchedEffect(gate) {
        if (gate.granted(Need.LocalNetwork)) model.appear() else gate.request(Need.LocalNetwork, fromUser = false)
    }
    DisposableEffect(model) { onDispose { model.disappear() } }
    PairContent(
        state = state,
        canGoBack = graph.navigator.backStack.size > 1,
        actions =
        PairActions(
            back = graph.navigator::back,
            scan = { gate.request(Need.Camera) },
            allowLocalNetwork = { PermissionGate.openAppSettings(context) },
            choose = model::choose,
            setAddress = model::setAddress,
            setCode = model::setCode,
            submitManual = model::submitManual,
            submitCode = model::submitCode,
            scanned = model::scanned,
            trust = model::trust,
            cancel = model::cancel,
            demo = { graph.demo.demo(true) },
        ),
    )
}

class PairActions(
    val back: () -> Unit,
    val scan: () -> Unit,
    val allowLocalNetwork: () -> Unit,
    val choose: (DiscoveredServer) -> Unit,
    val setAddress: (String) -> Unit,
    val setCode: (String) -> Unit,
    val submitManual: () -> Unit,
    val submitCode: () -> Unit,
    val scanned: (String) -> Unit,
    val trust: () -> Unit,
    val cancel: () -> Unit,
    val demo: () -> Unit,
)

@Composable
fun PairContent(
    state: PairUiState,
    canGoBack: Boolean,
    actions: PairActions,
) {
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.safeDrawingPadding().verticalScroll(rememberScrollState())) {
            TopBar(stringResource(R.string.pair_title), onBack = actions.back.takeIf { canGoBack })
            state.error?.let { error ->
                Text(
                    error.text(),
                    color = LocalStatusColors.current.danger,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
            }
            ScanSection(state, actions)
            NearbySection(state, actions)
            ManualSection(state, actions)
            SectionTitle(stringResource(R.string.demo))
            OutlinedButton(onClick = actions.demo, modifier = Modifier.padding(horizontal = 16.dp)) {
                Text(stringResource(R.string.demo_try))
            }
            Spacer(Modifier.height(BOTTOM_ROOM))
        }
        StepDialogs(state, actions)
    }
}

@Composable
private fun ScanSection(
    state: PairUiState,
    actions: PairActions,
) {
    SectionTitle(stringResource(R.string.pair_scan_section))
    Row(
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.padding(horizontal = 16.dp),
    ) {
        Button(onClick = actions.scan) { Text(stringResource(R.string.pair_scan)) }
        if (!state.cameraAllowed) {
            Text(stringResource(R.string.camera_off), color = LocalStatusColors.current.warn)
            TextButton(onClick = actions.scan) { Text(stringResource(R.string.allow)) }
        }
    }
}

@Composable
private fun NearbySection(
    state: PairUiState,
    actions: PairActions,
) {
    SectionTitle(stringResource(R.string.pair_nearby))
    when {
        state.browseError || !state.localNetworkAllowed ->
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 16.dp)) {
                Text(stringResource(R.string.local_network_off), color = LocalStatusColors.current.warn)
                TextButton(onClick = actions.allowLocalNetwork) { Text(stringResource(R.string.open_settings)) }
            }

        state.nearby.isEmpty() ->
            Text(
                stringResource(R.string.pair_none_found),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp),
            )

        else ->
            state.nearby.forEach { server ->
                ListItem(
                    headlineContent = { Text(server.name) },
                    supportingContent = { Text(server.hosts.firstOrNull() ?: "") },
                    trailingContent = { TextButton(onClick = { actions.choose(server) }) { Text(stringResource(R.string.pair_button)) } },
                    colors = ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.background),
                )
            }
    }
}

@Composable
private fun ManualSection(
    state: PairUiState,
    actions: PairActions,
) {
    SectionTitle(stringResource(R.string.pair_manual))
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(
            value = state.address,
            onValueChange = actions.setAddress,
            label = { Text(stringResource(R.string.pair_address_hint)) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            modifier = Modifier.fillMaxWidth(),
        )
        CodeField(state.code, actions.setCode)
        Button(onClick = actions.submitManual, enabled = state.canPairManually) {
            Text(stringResource(R.string.pair_button))
        }
    }
}

@Composable
private fun CodeField(
    code: String,
    setCode: (String) -> Unit,
) {
    OutlinedTextField(
        value = code,
        onValueChange = setCode,
        label = { Text(stringResource(R.string.pair_code)) },
        singleLine = true,
        visualTransformation = GroupedCode,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun StepDialogs(
    state: PairUiState,
    actions: PairActions,
) {
    when (val step = state.step) {
        PairStep.Scanning -> ScannerSheet(onPayload = actions.scanned, onClose = actions.cancel)
        is PairStep.Code -> CodeDialog(step.server, state, actions)
        is PairStep.Confirm -> TrustDialog(step.offer, actions)
        PairStep.Pairing -> PairingDialog()
        else -> Unit
    }
}

@Composable
private fun CodeDialog(
    server: DiscoveredServer,
    state: PairUiState,
    actions: PairActions,
) {
    AlertDialog(
        onDismissRequest = actions.cancel,
        title = { Text(server.name) },
        text = {
            Column {
                CodeField(state.code, actions.setCode)
                state.error?.let { Text(it.text(), color = LocalStatusColors.current.danger) }
            }
        },
        confirmButton = { TextButton(onClick = actions.submitCode) { Text(stringResource(R.string.pair_button)) } },
        dismissButton = { TextButton(onClick = actions.cancel) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun TrustDialog(
    offer: PairOffer,
    actions: PairActions,
) {
    AlertDialog(
        onDismissRequest = actions.cancel,
        title = { Text(stringResource(R.string.pair_trust_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                offer.serverName?.let { Text(it) }
                offer.fingerprintShort?.let { Text(stringResource(R.string.pair_key, it)) }
            }
        },
        confirmButton = { TextButton(onClick = actions.trust) { Text(stringResource(R.string.pair_trust)) } },
        dismissButton = { TextButton(onClick = actions.cancel) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun PairingDialog() {
    AlertDialog(
        onDismissRequest = {},
        title = { Text(stringResource(R.string.pairing)) },
        text = {
            Box(Modifier.fillMaxWidth().heightIn(min = 48.dp), contentAlignment = Alignment.Center) {
                CircularProgressIndicator()
            }
        },
        confirmButton = {},
    )
}

object GroupedCode : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val grouped = PairCode.grouped(text.text)
        val mapping =
            object : OffsetMapping {
                override fun originalToTransformed(offset: Int): Int = if (offset > GROUP) offset + 1 else offset

                override fun transformedToOriginal(offset: Int): Int = if (offset > GROUP) offset - 1 else offset
            }
        return TransformedText(AnnotatedString(grouped), mapping)
    }

    private const val GROUP = 4
}
