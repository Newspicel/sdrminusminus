package dev.newspicel.sdrmm.secrets

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import org.junit.After
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class KeystoreVaultDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val dir = File(context.noBackupFilesDir, "vault-device-test")

    @After
    fun cleanUp() {
        dir.deleteRecursively()
    }

    @Test
    fun a_new_instance_reads_what_the_last_one_stored() {
        KeystoreVault(dir, KeystoreCipher(ALIAS)).store("server/a", "secret".toByteArray())
        val reopened = KeystoreVault(dir, KeystoreCipher(ALIAS))
        assertThat(reopened.keys()).containsExactly("server/a")
        assertThat(reopened.load("server/a")).isEqualTo("secret".toByteArray())
        reopened.delete("server/a")
        assertThat(reopened.keys()).isEmpty()
        assertThat(reopened.load("server/a")).isNull()
    }

    private companion object {
        const val ALIAS = "sdrmm.test.vault"
    }
}
