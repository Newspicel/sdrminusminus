package dev.newspicel.sdrmm.pair

import com.google.zxing.BinaryBitmap
import com.google.zxing.ChecksumException
import com.google.zxing.DecodeHintType
import com.google.zxing.FormatException
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader

sealed interface QrResult {
    data class Payload(
        val text: String,
    ) : QrResult
}

object QrDecoder {
    private val HINTS = mapOf(DecodeHintType.TRY_HARDER to true)

    fun decode(
        luma: ByteArray,
        width: Int,
        height: Int,
        rowStride: Int,
        scratch: ByteArray,
    ): QrResult? {
        val data =
            if (rowStride == width) {
                luma
            } else {
                for (row in 0 until height) System.arraycopy(luma, row * rowStride, scratch, row * width, width)
                scratch
            }
        val source = PlanarYUVLuminanceSource(data, width, height, 0, 0, width, height, false)
        return try {
            QrResult.Payload(QRCodeReader().decode(BinaryBitmap(HybridBinarizer(source)), HINTS).text)
        } catch (_: NotFoundException) {
            null
        } catch (_: ChecksumException) {
            null
        } catch (_: FormatException) {
            null
        }
    }
}
