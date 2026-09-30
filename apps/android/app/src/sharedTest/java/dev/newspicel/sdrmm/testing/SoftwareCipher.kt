package dev.newspicel.sdrmm.testing

import dev.newspicel.sdrmm.secrets.GcmCipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

class SoftwareCipher : GcmCipher() {
    private val key: SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()

    override fun encryptionKey(): SecretKey = key

    override fun decryptionKey(): SecretKey = key
}
