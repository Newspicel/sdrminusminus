package dev.newspicel.sdrmm.secrets

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith
import java.security.GeneralSecurityException

@RunWith(AndroidJUnit4::class)
class KeystoreCipherDeviceTest {
    private val cipher = KeystoreCipher("sdrmm.test.cipher")
    private val aad = "server/a".toByteArray()

    @Test
    fun round_trip() {
        val blob = cipher.encrypt("token".toByteArray(), aad)
        assertThat(blob[0]).isEqualTo(GcmCipher.FORMAT)
        assertThat(cipher.decrypt(blob, aad)).isEqualTo("token".toByteArray())
        assertThat(cipher.encrypt("token".toByteArray(), aad)).isNotEqualTo(blob)
    }

    @Test
    fun tamper_is_refused() {
        val blob = cipher.encrypt("token".toByteArray(), aad)
        blob[blob.size - 2] = (blob[blob.size - 2].toInt() xor 0x40).toByte()
        assertThrows(GeneralSecurityException::class.java) { cipher.decrypt(blob, aad) }
    }

    @Test
    fun other_aad_is_refused() {
        val blob = cipher.encrypt("token".toByteArray(), aad)
        assertThrows(GeneralSecurityException::class.java) { cipher.decrypt(blob, "server/b".toByteArray()) }
    }
}
