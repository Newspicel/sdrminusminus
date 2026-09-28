package dev.newspicel.sdrmm.pair

import com.google.common.truth.Truth.assertThat
import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import dev.newspicel.sdrmm.testing.Samples
import org.junit.Test

class QrDecoderTest {
    private val size = 300

    private fun luma(
        text: String,
        stride: Int,
    ): ByteArray {
        val matrix = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, size, size)
        val out = ByteArray(stride * size) { 0x7f }
        for (y in 0 until size) {
            for (x in 0 until size) out[y * stride + x] = if (matrix[x, y]) 0 else 0xff.toByte()
        }
        return out
    }

    @Test
    fun roundtrip() {
        val link = Samples.link()
        val decoded = QrDecoder.decode(luma(link, size), size, size, size, ByteArray(size * size))
        assertThat(decoded).isEqualTo(QrResult.Payload(link))
    }

    @Test
    fun row_stride() {
        val link = Samples.link()
        val decoded = QrDecoder.decode(luma(link, size + 32), size, size, size + 32, ByteArray(size * size))
        assertThat(decoded).isEqualTo(QrResult.Payload(link))
    }

    @Test
    fun blank() {
        val blank = ByteArray(size * size) { 0xff.toByte() }
        assertThat(QrDecoder.decode(blank, size, size, size, ByteArray(size * size))).isNull()
    }
}
