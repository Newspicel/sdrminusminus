package dev.newspicel.sdrmm.secrets

import android.util.Log
import androidx.core.util.AtomicFile
import dev.newspicel.sdrmm.ffi.SecretVault
import dev.newspicel.sdrmm.ffi.VaultException
import java.io.File
import java.io.IOException
import java.security.GeneralSecurityException

class KeystoreVault(
    private val dir: File,
    private val cipher: SecretCipher,
) : SecretVault {
    @Synchronized
    override fun load(key: String): ByteArray? {
        val file = fileFor(key)
        if (!file.exists()) return null
        val blob = io { AtomicFile(file).readFully() }
        return crypto { cipher.decrypt(blob, key.toByteArray(Charsets.UTF_8)) }
    }

    @Synchronized
    override fun store(
        key: String,
        value: ByteArray,
    ) {
        val blob = crypto { cipher.encrypt(value, key.toByteArray(Charsets.UTF_8)) }
        io {
            if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Cannot create ${dir.path}")
            val file = AtomicFile(fileFor(key))
            val out = file.startWrite()
            try {
                out.write(blob)
                file.finishWrite(out)
            } catch (error: IOException) {
                file.failWrite(out)
                throw error
            }
        }
    }

    @Synchronized
    override fun delete(key: String) {
        io { AtomicFile(fileFor(key)).delete() }
    }

    @Synchronized
    override fun keys(): List<String> {
        val names = io { dir.list()?.toList() ?: emptyList() }
        return names.mapNotNull(::keyOf).sorted()
    }

    private fun fileFor(key: String): File = File(dir, key.toByteArray(Charsets.UTF_8).toHex() + SUFFIX)

    private inline fun <T> io(block: () -> T): T = try {
        block()
    } catch (error: IOException) {
        Log.w(TAG, "Vault file access failed", error)
        throw VaultException.Os(IO_STATUS)
    }

    private inline fun <T> crypto(block: () -> T): T = try {
        block()
    } catch (error: GeneralSecurityException) {
        Log.w(TAG, "Vault item unreadable: ${error.javaClass.simpleName}")
        throw VaultException.Corrupt()
    }

    companion object {
        const val IO_STATUS = -5
        private const val SUFFIX = ".bin"

        fun keyOf(fileName: String): String? {
            if (!fileName.endsWith(SUFFIX)) return null
            val hex = fileName.removeSuffix(SUFFIX)
            if (hex.isEmpty() || hex.length % 2 != 0 || hex.any { it !in HEX_DIGITS }) return null
            val bytes = ByteArray(hex.length / 2) { index -> hex.substring(index * 2, index * 2 + 2).toInt(16).toByte() }
            return bytes.toString(Charsets.UTF_8)
        }

        private const val HEX_DIGITS = "0123456789abcdef"
        private const val TAG = "SdrmmVault"

        private fun ByteArray.toHex(): String = buildString(size * 2) {
            for (byte in this@toHex) {
                val value = byte.toInt() and 0xff
                append(HEX_DIGITS[value shr 4])
                append(HEX_DIGITS[value and 0x0f])
            }
        }
    }
}
