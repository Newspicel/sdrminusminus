package dev.newspicel.sdrmm.pair

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.testing.FakeCoreGateway
import dev.newspicel.sdrmm.testing.FakeDiscovery
import dev.newspicel.sdrmm.testing.FakeSettingsStore
import dev.newspicel.sdrmm.testing.MainDispatcherRule
import dev.newspicel.sdrmm.testing.Samples
import dev.newspicel.sdrmm.testing.TestAppGraph
import dev.newspicel.sdrmm.ui.Destination
import dev.newspicel.sdrmm.ui.components.UiText
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PairViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val core = FakeCoreGateway()
    private val settings = FakeSettingsStore()
    private val discovery = FakeDiscovery()
    private val intake = PairLinkIntake()
    private val graph by lazy { TestAppGraph.create(ApplicationProvider.getApplicationContext(), core, settings, discovery) }
    private val navigator get() = graph.navigator
    private val model by lazy {
        PairViewModel(core, settings, discovery, intake, navigator, graph::activate).also { core.calls.clear() }
    }

    @Test
    fun qr_needs_trust() {
        model.scan()
        assertThat(model.state.value.step).isEqualTo(PairStep.Scanning)
        model.scanned(Samples.link())
        assertThat(model.state.value.step).isEqualTo(PairStep.Confirm(Samples.offer()))
        assertThat(core.calls.none { it.startsWith("pair:") }).isTrue()
        model.cancel()
        assertThat(model.state.value.step).isEqualTo(PairStep.Choose)
        assertThat(core.calls.none { it.startsWith("pair:") }).isTrue()
    }

    @Test
    fun bad_qr() {
        model.scanned("hello")
        assertThat(model.state.value.error).isEqualTo(UiText.Res(R.string.err_bad_qr))
        assertThat(model.state.value.step).isEqualTo(PairStep.Choose)
        assertThat(core.calls).isEmpty()
    }

    @Test
    fun a_refused_link_shows_the_core_error() {
        core.outcomes["parsePairLink"] = Outcome.Failed(CoreException.InvalidLink("bad key"))
        model.open(Samples.link())
        assertThat(model.state.value.error).isEqualTo(UiText.Res(R.string.err_bad_qr))
    }

    @Test
    fun code_is_eight_digits() {
        model.choose(Samples.discovered())
        model.setCode("1234567")
        model.submitCode()
        assertThat(model.state.value.error).isEqualTo(UiText.Res(R.string.code_digits))
        assertThat(core.calls).isEmpty()
        model.setCode("4821 0937")
        assertThat(model.state.value.code).isEqualTo("48210937")
        model.submitCode()
        assertThat(core.calls).containsExactly("offerFromDiscovery:SDR-- bench")
        assertThat(model.state.value.step).isEqualTo(PairStep.Confirm(Samples.offer(code = "48210937")))
    }

    @Test
    fun manual_needs_address_and_code() {
        model.setCode("48210937")
        assertThat(model.state.value.canPairManually).isFalse()
        model.setAddress(" 10.0.2.2:8443 ")
        assertThat(model.state.value.canPairManually).isTrue()
        model.submitManual()
        assertThat(core.calls).containsExactly("offerManual:10.0.2.2:8443")
        assertThat(model.state.value.step).isInstanceOf(PairStep.Confirm::class.java)
    }

    @Test
    fun trust_pairs_saves_connects() {
        model.open(Samples.link())
        model.trust()
        assertThat(core.calls).containsAtLeast("pair:Pixel Test", "connect:s1").inOrder()
        assertThat(settings.settings.value.activeServerId).isEqualTo("s1")
        assertThat(model.state.value.step).isEqualTo(PairStep.Done(Samples.server()))
        assertThat(navigator.backStack).containsExactly(Destination.Missions)
    }

    @Test
    fun wrong_code_label() {
        core.outcomes["pair"] = Outcome.Failed(CoreException.WrongCode())
        model.open(Samples.link())
        model.trust()
        assertThat(model.state.value.error).isEqualTo(UiText.Res(R.string.err_wrong_code))
        assertThat(model.state.value.step).isEqualTo(PairStep.Choose)
        assertThat(settings.settings.value.activeServerId).isNull()
        assertThat(navigator.backStack).containsExactly(Destination.Pair)
    }

    @Test
    fun a_deep_link_opens_the_trust_step() {
        model.cancel()
        intake.offer(Samples.link())
        assertThat(model.state.value.step).isEqualTo(PairStep.Confirm(Samples.offer()))
        assertThat(intake.link.value).isNull()
    }

    @Test
    fun nearby_follows_discovery() {
        model.appear()
        assertThat(discovery.running).isEqualTo(1)
        discovery.found.value = listOf(Samples.discovered())
        discovery.failed.value = true
        assertThat(model.state.value.nearby).containsExactly(Samples.discovered())
        assertThat(model.state.value.browseError).isTrue()
        model.disappear()
        assertThat(discovery.running).isEqualTo(0)
    }
}
