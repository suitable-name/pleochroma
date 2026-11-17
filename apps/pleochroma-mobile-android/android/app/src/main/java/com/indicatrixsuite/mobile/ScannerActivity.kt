package com.indicatrixsuite.mobile

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Scans the QR code of a pairing link (CameraX preview, ZXing decoder).
 *
 * [MainActivity.startQrScan] starts it for a result. The camera permission is asked for only
 * here, when the user taps Scan; without it the screen says so in plain words and the user
 * can paste the link instead. A code is accepted only when its text starts with
 * `pleochroma://pair`; anything else is ignored and scanning goes on. The accepted text
 * goes back in [EXTRA_LINK]; it holds a one-time pairing code, so it is never logged.
 *
 * Nothing is recorded: frames are decoded in memory and dropped, and the camera is released
 * as soon as the screen leaves.
 */
class ScannerActivity : ComponentActivity() {

    private val finished = AtomicBoolean(false)
    private val reader = MultiFormatReader().apply {
        setHints(mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE)))
    }
    private var analysisExecutor: ExecutorService? = null
    private var previewView: PreviewView? = null
    private lateinit var message: TextView

    private val askForCamera =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) {
                startCamera()
            } else {
                showMessage(
                    "Camera access is off for Pleochroma. Turn it on in the Android " +
                        "settings of this app, or go back and paste the pairing link instead."
                )
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setResult(Activity.RESULT_CANCELED)
        setContentView(buildContent())
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            startCamera()
        } else {
            askForCamera.launch(Manifest.permission.CAMERA)
        }
    }

    override fun onDestroy() {
        analysisExecutor?.shutdown()
        analysisExecutor = null
        super.onDestroy()
    }

    // ---------------------------------------------------------------------------------
    // Screen
    // ---------------------------------------------------------------------------------

    /** A black screen: the camera picture, a sentence, and a Close button of 48 dp or more. */
    private fun buildContent(): View {
        val root = FrameLayout(this).apply { setBackgroundColor(Color.BLACK) }
        val preview = PreviewView(this).apply {
            implementationMode = PreviewView.ImplementationMode.COMPATIBLE
            scaleType = PreviewView.ScaleType.FILL_CENTER
        }
        previewView = preview
        root.addView(
            preview,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ),
        )

        message = TextView(this).apply {
            text = "Point the camera at the QR code on the computer."
            setTextColor(Color.WHITE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            typeface = Typeface.DEFAULT
            gravity = Gravity.CENTER
            setBackgroundColor(0x99000000.toInt())
            val pad = dp(16)
            setPadding(pad, pad, pad, pad)
        }
        val close = Button(this).apply {
            text = "Close"
            minimumHeight = dp(48)
            minHeight = dp(48)
            contentDescription = "Close the scanner"
            setOnClickListener { finish() }
        }
        val bottom = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(0x99000000.toInt())
            addView(message, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
            addView(
                close,
                LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply {
                    val side = dp(16)
                    setMargins(side, 0, side, side)
                },
            )
        }
        root.addView(
            bottom,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM,
            ),
        )
        // Edge to edge: keep the controls out of the system bars.
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }
        return root
    }

    private fun dp(value: Int): Int =
        TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_DIP,
            value.toFloat(),
            resources.displayMetrics,
        ).toInt()

    private fun showMessage(text: String) {
        message.text = text
        previewView?.visibility = View.INVISIBLE
    }

    // ---------------------------------------------------------------------------------
    // Camera
    // ---------------------------------------------------------------------------------

    private fun startCamera() {
        val future = ProcessCameraProvider.getInstance(this)
        future.addListener(
            {
                try {
                    bind(future.get())
                } catch (e: Exception) {
                    showMessage("The camera could not be started. Go back and paste the pairing link instead.")
                }
            },
            ContextCompat.getMainExecutor(this),
        )
    }

    private fun bind(provider: ProcessCameraProvider) {
        val view = previewView ?: return
        val preview = Preview.Builder().build().also { it.setSurfaceProvider(view.surfaceProvider) }
        val executor = analysisExecutor ?: Executors.newSingleThreadExecutor().also { analysisExecutor = it }
        val analysis = ImageAnalysis.Builder()
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
        analysis.setAnalyzer(executor) { image -> analyze(image) }
        provider.unbindAll()
        provider.bindToLifecycle(this, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
    }

    /** Decodes the luminance plane of one frame; a frame without a code is dropped. */
    private fun analyze(image: ImageProxy) {
        try {
            if (finished.get()) return
            val plane = image.planes[0]
            val buffer = plane.buffer
            // The last row of a plane can be shorter than its stride; pad so ZXing can read it.
            val data = ByteArray(maxOf(buffer.remaining(), plane.rowStride * image.height))
            buffer.get(data, 0, buffer.remaining())
            val source = PlanarYUVLuminanceSource(
                data, plane.rowStride, image.height, 0, 0, image.width, image.height, false,
            )
            val text = try {
                reader.decodeWithState(BinaryBitmap(HybridBinarizer(source))).text
            } catch (e: NotFoundException) {
                null
            } finally {
                reader.reset()
            }
            if (text != null && text.startsWith(LINK_PREFIX, ignoreCase = true)) {
                accept(text)
            }
        } catch (e: Exception) {
            // A frame that cannot be read is skipped; the next one is tried.
        } finally {
            image.close()
        }
    }

    /** Hands the text to [MainActivity] once and leaves. */
    private fun accept(text: String) {
        if (!finished.compareAndSet(false, true)) return
        runOnUiThread {
            setResult(Activity.RESULT_OK, Intent().putExtra(EXTRA_LINK, text))
            finish()
        }
    }

    companion object {
        /** The result extra that carries the scanned `pleochroma://pair?...` text. */
        const val EXTRA_LINK = "pleochroma.pairing_link"

        /** A scanned code is accepted only when it starts with this. */
        private const val LINK_PREFIX = "pleochroma://pair"
    }
}
