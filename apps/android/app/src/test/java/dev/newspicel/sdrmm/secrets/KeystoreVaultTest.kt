package dev.newspicel.sdrmm.secrets

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.ffi.VaultException
import dev.newspicel.sdrmm.testing.SoftwareCipher
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class KeystoreVaultTest {
    @get:Rule val folder = TemporaryFolder()

    private val cipher = SoftwareCipher()

    private fun vault(dir: File = File(folder.root, "vault")) = KeystoreVault(dir, cipher)

    @Test
    fun store_load_delete() {
        val vault = vault()
        assertThat(vault.keys()).isEmpty()
        assertThat(vault.load("server/b")).isNull()
        vault.store("server/b", "two".toByteArray())
        vault.store("server/a", "one".toByteArray())
        vault.store("server/ü", byteArrayOf(0, 1, 2))
        assertThat(vault.keys()).containsExactly("server/a", "server/b", "server/ü").inOrder()
        assertThat(vault.load("server/a")?.toString(Charsets.UTF_8)).isEqualTo("one")
        assertThat(vault().load("server/ü")).isEqualTo(byteArrayOf(0, 1, 2))
        vault.store("server/a", "uno".toByteArray())
        assertThat(vault.load("server/a")?.toString(Charsets.UTF_8)).isEqualTo("uno")
        vault.delete("server/a")
        vault.delete("server/missing")
        assertThat(vault.keys()).containsExactly("server/b", "server/ü").inOrder()
        assertThat(vault.load("server/a")).isNull()
    }

    @Test
    fun tamper_is_corrupt() {
        val dir = File(folder.root, "vault")
        val vault = vault(dir)
        vault.store("server/a", "secret".toByteArray())
        val file = dir.listFiles().orEmpty().single()
        val bytes = file.readBytes()
        bytes[bytes.size - 1] = (bytes[bytes.size - 1].toInt() xor 0x01).toByte()
        file.writeBytes(bytes)
        assertThrows(VaultException.Corrupt::class.java) { vault.load("server/a") }
        assertThat(file.exists()).isTrue()
        assertThat(vault.keys()).containsExactly("server/a")
    }

    @Test
    fun renamed_is_corrupt() {
        val dir = File(folder.root, "vault")
        val vault = vault(dir)
        vault.store("server/a", "secret".toByteArray())
        val original = dir.listFiles().orEmpty().single()
        original.copyTo(File(dir, "7365727665722f62.bin"))
        assertThat(vault.keys()).containsExactly("server/a", "server/b").inOrder()
        assertThrows(VaultException.Corrupt::class.java) { vault.load("server/b") }
    }

    @Test
    fun an_unwritable_dir_is_an_os_error() {
        val blocker = folder.newFile("vault")
        val error = assertThrows(VaultException.Os::class.java) { vault(blocker).store("server/a", byteArrayOf(1)) }
        assertThat(error.status).isEqualTo(KeystoreVault.IO_STATUS)
    }

    @Test
    fun foreign_file_names_are_not_keys() {
        assertThat(KeystoreVault.keyOf("7365727665722f61.bin")).isEqualTo("server/a")
        assertThat(KeystoreVault.keyOf("7365727665722f61.bin.new")).isNull()
        assertThat(KeystoreVault.keyOf("zz.bin")).isNull()
        assertThat(KeystoreVault.keyOf("abc.bin")).isNull()
        assertThat(KeystoreVault.keyOf(".bin")).isNull()
    }

    @Test
    fun the_blob_has_the_documented_layout() {
        val blob = cipher.encrypt("hi".toByteArray(), "k".toByteArray())
        assertThat(blob[0]).isEqualTo(GcmCipher.FORMAT)
        assertThat(blob.size).isEqualTo(1 + GcmCipher.IV_BYTES + 2 + GcmCipher.TAG_BITS / 8)
        assertThat(cipher.decrypt(blob, "k".toByteArray())).isEqualTo("hi".toByteArray())
    }
}
