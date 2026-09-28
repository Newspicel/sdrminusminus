package dev.newspicel.sdrmm.pair

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.CoreGateway
import dev.newspicel.sdrmm.core.ErrorText
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.DiscoveredServer
import dev.newspicel.sdrmm.ffi.PairOffer
import dev.newspicel.sdrmm.ffi.SavedServer
import dev.newspicel.sdrmm.settings.SettingsStore
import dev.newspicel.sdrmm.ui.AppNavigator
import dev.newspicel.sdrmm.ui.components.UiText
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

sealed interface PairStep {
    data object Choose : PairStep

    data object Scanning : PairStep

    data class Code(
        val server: DiscoveredServer,
    ) : PairStep

    data class Confirm(
        val offer: PairOffer,
    ) : PairStep

    data object Pairing : PairStep

    data class Done(
        val server: SavedServer,
    ) : PairStep
}

data class PairUiState(
    val step: PairStep = PairStep.Choose,
    val nearby: List<DiscoveredServer> = emptyList(),
    val browseError: Boolean = false,
    val localNetworkAllowed: Boolean = true,
    val cameraAllowed: Boolean = true,
    val address: String = "",
    val code: String = "",
    val error: UiText? = null,
) {
    val canPairManually: Boolean get() = address.isNotBlank() && PairCode.valid(code)
}

class PairViewModel(
    private val core: CoreGateway,
    private val settings: SettingsStore,
    private val discovery: Discovery,
    private val intake: PairLinkIntake,
    private val navigator: AppNavigator,
    private val activate: (String) -> Unit,
) : ViewModel() {
    private val ui = MutableStateFlow(PairUiState())
    val state: StateFlow<PairUiState> = ui.asStateFlow()

    init {
        viewModelScope.launch {
            combine(discovery.found, discovery.failed) { found, failed -> found to failed }
                .collect { (found, failed) -> ui.update { it.copy(nearby = found, browseError = failed) } }
        }
        viewModelScope.launch {
            intake.link.collect { link -> if (link != null) intake.take()?.let(::open) }
        }
    }

    fun appear() {
        discovery.start()
    }

    fun disappear() {
        discovery.stop()
    }

    fun setLocalNetworkAllowed(allowed: Boolean) {
        ui.update { it.copy(localNetworkAllowed = allowed) }
    }

    fun setCameraAllowed(allowed: Boolean) {
        ui.update { it.copy(cameraAllowed = allowed) }
    }

    fun scan() {
        ui.update { it.copy(step = PairStep.Scanning, error = null) }
    }

    fun scanned(payload: String) {
        if (!PairLinkIntake.isPairLink(payload)) {
            fail(UiText.Res(R.string.err_bad_qr))
            return
        }
        offered(core.parsePairLink(payload))
    }

    fun open(uri: String) {
        scanned(uri)
    }

    fun choose(server: DiscoveredServer) {
        ui.update { it.copy(step = PairStep.Code(server), code = "", error = null) }
    }

    fun setAddress(value: String) {
        ui.update { it.copy(address = value.trim(), error = null) }
    }

    fun setCode(value: String) {
        ui.update { it.copy(code = PairCode.clean(value), error = null) }
    }

    fun submitCode() {
        val step = ui.value.step as? PairStep.Code ?: return
        val code = ui.value.code
        if (!PairCode.valid(code)) {
            ui.update { it.copy(error = UiText.Res(R.string.code_digits)) }
            return
        }
        offered(core.offerFromDiscovery(step.server, code))
    }

    fun submitManual() {
        val current = ui.value
        if (!PairCode.valid(current.code)) {
            ui.update { it.copy(error = UiText.Res(R.string.code_digits)) }
            return
        }
        if (current.address.isBlank()) return
        ui.update { it.copy(step = PairStep.Pairing, error = null) }
        viewModelScope.launch { offered(core.offerManual(current.address, current.code)) }
    }

    fun trust() {
        val offer = (ui.value.step as? PairStep.Confirm)?.offer ?: return
        ui.update { it.copy(step = PairStep.Pairing, error = null) }
        viewModelScope.launch {
            when (val paired = core.pair(offer, settings.current().phoneName)) {
                is Outcome.Ok -> done(paired.value)
                is Outcome.Failed -> fail(ErrorText.short(paired.error))
            }
        }
    }

    fun cancel() {
        ui.update { it.copy(step = PairStep.Choose, error = null) }
    }

    private fun done(server: SavedServer) {
        activate(server.id)
        ui.update { it.copy(step = PairStep.Done(server), code = "", address = "") }
        navigator.paired()
    }

    private fun offered(outcome: Outcome<PairOffer>) {
        when (outcome) {
            is Outcome.Ok -> ui.update { it.copy(step = PairStep.Confirm(outcome.value), error = null) }
            is Outcome.Failed -> fail(ErrorText.short(outcome.error))
        }
    }

    private fun fail(error: UiText) {
        ui.update { it.copy(step = PairStep.Choose, error = error) }
    }
}

object PairCode {
    const val DIGITS = 8
    private val VALID = Regex("^[0-9]{$DIGITS}$")

    fun clean(input: String): String = input.filter { it in '0'..'9' }.take(DIGITS)

    fun valid(code: String): Boolean = VALID.matches(code)

    fun grouped(code: String): String = code.chunked(4).joinToString(" ")
}
