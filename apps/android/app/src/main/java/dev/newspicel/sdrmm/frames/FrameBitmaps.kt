package dev.newspicel.sdrmm.frames

import android.graphics.Bitmap
import androidx.core.graphics.createBitmap
import dev.newspicel.sdrmm.core.Outcome
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ffi.RgbaImage
import java.nio.ByteBuffer

class FrameBitmaps {
    private val pair = arrayOfNulls<Bitmap>(2)
    private var next = 0

    fun update(image: RgbaImage): Outcome<Bitmap> {
        val width = image.width.toInt()
        val height = image.height.toInt()
        if (width <= 0 || height <= 0 || image.rgba.size.toLong() != width.toLong() * height * BYTES) {
            return Outcome.Failed(CoreException.Internal("bad image"))
        }
        val slot = next
        next = 1 - next
        val bitmap = pair[slot]?.takeIf { it.width == width && it.height == height } ?: createBitmap(width, height).also { pair[slot] = it }
        bitmap.copyPixelsFromBuffer(ByteBuffer.wrap(image.rgba))
        return Outcome.Ok(bitmap)
    }

    private companion object {
        const val BYTES = 4
    }
}
