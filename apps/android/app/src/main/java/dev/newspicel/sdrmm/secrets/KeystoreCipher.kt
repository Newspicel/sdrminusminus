package dev.newspicel.sdrmm.secrets

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import java.security.KeyStoreException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

class KeystoreCipher(
    private val alias: String = "sdrmm.vault.v1",
) : GcmCipher() {
    override fun encryptionKey(): SecretKey = stored() ?: generate()

    override fun decryptionKey(): SecretKey = stored() ?: throw KeyStoreException("Vault key missing")

    private fun stored(): SecretKey? {
        val store = KeyStore.getInstance(PROVIDER)
        store.load(null)
        return store.getKey(alias, null) as? SecretKey
    }

    private fun generate(): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(
            KeyGenParameterSpec
                .Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(KEY_BITS)
                .setRandomizedEncryptionRequired(true)
                .setUserAuthenticationRequired(false)
                .build(),
        )
        return generator.generateKey()
    }

    private companion object {
        const val PROVIDER = "AndroidKeyStore"
        const val KEY_BITS = 256
    }
}
