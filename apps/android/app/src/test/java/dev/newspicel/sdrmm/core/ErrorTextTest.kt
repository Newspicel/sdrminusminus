package dev.newspicel.sdrmm.core

import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ui.components.UiText
import org.junit.Test

class ErrorTextTest {
    private val every: List<Pair<CoreException, Int>> =
        listOf(
            CoreException.InvalidLink("no host") to R.string.err_bad_qr,
            CoreException.WrongCode() to R.string.err_wrong_code,
            CoreException.CodeExpired() to R.string.err_code_expired,
            CoreException.ProtocolMismatch(2u, 3u) to R.string.err_server_old,
            CoreException.ProtocolMismatch(4u, 3u) to R.string.err_app_old,
            CoreException.Unreachable(listOf("10.0.0.2:8443")) to R.string.err_no_answer,
            CoreException.LocalNetworkBlocked() to R.string.err_local_network,
            CoreException.KeyMismatch() to R.string.err_key_mismatch,
            CoreException.Revoked() to R.string.err_revoked,
            CoreException.Vault(-5) to R.string.err_keystore,
            CoreException.Server(500u, "boom") to R.string.err_server,
            CoreException.NotConnected() to R.string.err_offline,
            CoreException.NoMission() to R.string.err_no_mission,
            CoreException.Refused("Busy") to R.string.err_refused,
            CoreException.Internal("panic") to R.string.err_core,
        )

    @Test
    fun every_error_has_label() {
        for ((error, label) in every) {
            assertThat(ErrorText.label(error)).isEqualTo(label)
            assertThat(ErrorText.detail(error)).isNotEmpty()
        }
        val variants = CoreException::class.java.declaredClasses.filter { CoreException::class.java.isAssignableFrom(it) }
        assertThat(every.map { it.first.javaClass }.toSet()).containsExactlyElementsIn(variants)
    }

    @Test
    fun short_text_carries_server_values() {
        assertThat(ErrorText.short(CoreException.Server(503u, "down"))).isEqualTo(UiText.Res(R.string.err_server, listOf(503)))
        assertThat(ErrorText.short(CoreException.Refused("Busy"))).isEqualTo(UiText.Res(R.string.err_refused, listOf("Busy")))
        assertThat(ErrorText.short(CoreException.WrongCode())).isEqualTo(UiText.Res(R.string.err_wrong_code))
    }

    @Test
    fun detail_keeps_the_full_text() {
        assertThat(ErrorText.detail(CoreException.Server(500u, "disk full"))).isEqualTo("Server error 500: disk full")
        assertThat(ErrorText.detail(CoreException.Unreachable(listOf("a:1", "b:2")))).isEqualTo("No answer from a:1, b:2")
        assertThat(ErrorText.detail(CoreException.ProtocolMismatch(2u, 3u))).isEqualTo("Server protocol 2, app protocol 3")
    }
}
