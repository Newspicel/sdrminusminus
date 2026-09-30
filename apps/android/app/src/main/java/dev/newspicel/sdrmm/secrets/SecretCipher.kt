package dev.newspicel.sdrmm.secrets

import java.security.GeneralSecurityException
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

interface SecretCipher {
    fun encrypt(
        plain: ByteArray,
        aad: ByteArray,
    ): ByteArray

    fun decrypt(
        blob: ByteArray,
        aad: ByteArray,
    ): ByteArray
}

abstract class GcmCipher : SecretCipher {
    protected abstract fun encryptionKey(): SecretKey

    protected abstract fun decryptionKey(): SecretKey

    override fun encrypt(
        plain: ByteArray,
        aad: ByteArray,
    ): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, encryptionKey())
        cipher.updateAAD(aad)
        val body = cipher.doFinal(plain)
        val iv = cipher.iv
        if (iv.size != IV_BYTES) throw GeneralSecurityException("IV of ${iv.size} bytes")
        return byteArrayOf(FORMAT) + iv + body
    }

    override fun decrypt(
        blob: ByteArray,
        aad: ByteArray,
    ): ByteArray {
        if (blob.size < HEADER_BYTES + TAG_BYTES) throw GeneralSecurityException("Blob of ${blob.size} bytes")
        if (blob[0] != FORMAT) throw GeneralSecurityException("Blob format ${blob[0]}")
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, decryptionKey(), GCMParameterSpec(TAG_BITS, blob, 1, IV_BYTES))
        cipher.updateAAD(aad)
        return cipher.doFinal(blob, HEADER_BYTES, blob.size - HEADER_BYTES)
    }

    companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val FORMAT: Byte = 0x01
        const val IV_BYTES = 12
        const val TAG_BITS = 128
        private const val TAG_BYTES = TAG_BITS / 8
        private const val HEADER_BYTES = 1 + IV_BYTES
    }
}
