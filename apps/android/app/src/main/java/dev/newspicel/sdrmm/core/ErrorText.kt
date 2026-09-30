package dev.newspicel.sdrmm.core

import androidx.annotation.StringRes
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ui.components.UiText

object ErrorText {
    @StringRes
    fun label(error: CoreException): Int = when (error) {
        is CoreException.InvalidLink -> {
            R.string.err_bad_qr
        }

        is CoreException.WrongCode -> {
            R.string.err_wrong_code
        }

        is CoreException.CodeExpired -> {
            R.string.err_code_expired
        }

        is CoreException.ProtocolMismatch -> {
            if (error.server < error.app) R.string.err_server_old else R.string.err_app_old
        }

        is CoreException.Unreachable -> {
            R.string.err_no_answer
        }

        is CoreException.LocalNetworkBlocked -> {
            R.string.err_local_network
        }

        is CoreException.KeyMismatch -> {
            R.string.err_key_mismatch
        }

        is CoreException.Revoked -> {
            R.string.err_revoked
        }

        is CoreException.Vault -> {
            R.string.err_keystore
        }

        is CoreException.Server -> {
            R.string.err_server
        }

        is CoreException.NotConnected -> {
            R.string.err_offline
        }

        is CoreException.NoMission -> {
            R.string.err_no_mission
        }

        is CoreException.Refused -> {
            R.string.err_refused
        }

        is CoreException.Internal -> {
            R.string.err_core
        }
    }

    fun short(error: CoreException): UiText.Res = when (error) {
        is CoreException.Server -> UiText.Res(label(error), listOf(error.status.toInt()))
        is CoreException.Refused -> UiText.Res(label(error), listOf(error.text))
        else -> UiText.Res(label(error))
    }

    fun detail(error: CoreException): String = when (error) {
        is CoreException.InvalidLink -> error.reason
        is CoreException.WrongCode -> "Wrong code"
        is CoreException.CodeExpired -> "Code expired"
        is CoreException.ProtocolMismatch -> "Server protocol ${error.server}, app protocol ${error.app}"
        is CoreException.Unreachable -> "No answer from ${error.hosts.joinToString(", ")}"
        is CoreException.LocalNetworkBlocked -> "Local network permission off"
        is CoreException.KeyMismatch -> "Server key changed"
        is CoreException.Revoked -> "Phone removed on the server. Pair again"
        is CoreException.Vault -> "Keystore status ${error.status}"
        is CoreException.Server -> "Server error ${error.status}: ${error.text}"
        is CoreException.NotConnected -> "Not connected"
        is CoreException.NoMission -> "No mission open"
        is CoreException.Refused -> error.text
        is CoreException.Internal -> error.text
    }
}
