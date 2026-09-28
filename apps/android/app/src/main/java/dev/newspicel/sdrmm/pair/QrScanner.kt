package dev.newspicel.sdrmm.pair

import android.content.Context
import android.util.Size
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.lifecycle.awaitInstance
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class QrScanner(
    private val context: Context,
) {
    private val requests = MutableStateFlow<SurfaceRequest?>(null)
    private val payloads = MutableSharedFlow<QrResult>(extraBufferCapacity = 1)
    private val problem = MutableStateFlow<String?>(null)
    private var provider: ProcessCameraProvider? = null
    private var analysis: ImageAnalysis? = null
    private var executor: ExecutorService? = null
    private var luma = ByteArray(0)
    private var scratch = ByteArray(0)

    val surfaceRequests: StateFlow<SurfaceRequest?> = requests.asStateFlow()
    val results: SharedFlow<QrResult> = payloads.asSharedFlow()
    val failure: StateFlow<String?> = problem.asStateFlow()

    suspend fun bind(owner: LifecycleOwner) {
        try {
            val cameras = ProcessCameraProvider.awaitInstance(context)
            val preview = Preview.Builder().build().apply { setSurfaceProvider { request -> requests.value = request } }
            val analyzer = Executors.newSingleThreadExecutor()
            val images =
                ImageAnalysis
                    .Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .setOutputImageFormat(ImageAnalysis.OUTPUT_IMAGE_FORMAT_YUV_420_888)
                    .setResolutionSelector(
                        ResolutionSelector
                            .Builder()
                            .setResolutionStrategy(
                                ResolutionStrategy(Size(WIDTH, HEIGHT), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER),
                            ).build(),
                    ).build()
            images.setAnalyzer(analyzer, ::analyze)
            cameras.unbindAll()
            cameras.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, images)
            provider = cameras
            analysis = images
            executor = analyzer
            problem.value = null
        } catch (error: IllegalArgumentException) {
            problem.value = error.message ?: error.javaClass.simpleName
        } catch (error: IllegalStateException) {
            problem.value = error.message ?: error.javaClass.simpleName
        } catch (error: UnsupportedOperationException) {
            problem.value = error.message ?: error.javaClass.simpleName
        }
    }

    fun unbind() {
        analysis?.clearAnalyzer()
        provider?.unbindAll()
        executor?.shutdown()
        analysis = null
        provider = null
        executor = null
        requests.value = null
    }

    private fun analyze(image: ImageProxy) {
        image.use {
            val plane = it.planes[0]
            val buffer = plane.buffer
            if (luma.size < buffer.remaining()) luma = ByteArray(buffer.remaining())
            buffer.get(luma, 0, buffer.remaining())
            val size = it.width * it.height
            if (scratch.size < size) scratch = ByteArray(size)
            val found = QrDecoder.decode(luma, it.width, it.height, plane.rowStride, scratch) ?: return
            analysis?.clearAnalyzer()
            payloads.tryEmit(found)
        }
    }

    private companion object {
        const val WIDTH = 1280
        const val HEIGHT = 720
    }
}
