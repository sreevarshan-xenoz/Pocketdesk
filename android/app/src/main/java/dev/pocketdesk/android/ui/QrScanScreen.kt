package dev.pocketdesk.android.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import dev.pocketdesk.android.R
import dev.pocketdesk.android.pair.firstPairingCode
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import zxingcpp.BarcodeReader

/**
 * Scans a Pocketdesk pairing code with the camera (CameraX preview and
 * zxing-cpp, all free software, nothing sent anywhere) and hands the first
 * `pocketdesk://pair` value to [onCode].
 */
@Composable
fun QrScanScreen(onCode: (String) -> Unit, onBack: () -> Unit) {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
    val askCamera =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) askCamera.launch(Manifest.permission.CAMERA) }

    if (!granted) {
        Column(
            Modifier.fillMaxSize().padding(32.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(stringResource(R.string.scan_need_camera), textAlign = TextAlign.Center)
            Spacer(Modifier.height(16.dp))
            Button(onClick = { askCamera.launch(Manifest.permission.CAMERA) }) {
                Text(stringResource(R.string.scan_allow_camera))
            }
            TextButton(onClick = onBack) { Text(stringResource(R.string.exit)) }
        }
        return
    }

    Box(Modifier.fillMaxSize()) {
        CameraPreview(onCode)
        Column(
            Modifier.align(Alignment.BottomCenter).padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                stringResource(R.string.scan_hint),
                color = Color.White,
                style = MaterialTheme.typography.bodyLarge,
                textAlign = TextAlign.Center,
            )
            TextButton(onClick = onBack) { Text(stringResource(R.string.exit), color = Color.White) }
        }
    }
}

@Composable
private fun CameraPreview(onCode: (String) -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val analysisExecutor = remember { Executors.newSingleThreadExecutor() }
    // A code on a monitor: glare, moiré and a dark theme (inverted) are the
    // hard cases, hence tryHarder and tryInvert.
    val reader = remember {
        BarcodeReader(
            BarcodeReader.Options(
                formats = setOf(BarcodeReader.Format.QR_CODE),
                tryHarder = true,
                tryInvert = true,
                tryDownscale = true,
            ),
        )
    }
    val delivered = remember { AtomicBoolean(false) }
    // The camera binds to the Activity, which outlives this screen: without
    // an explicit unbind it stays on (and keeps analysing into a closed
    // scanner) after the user leaves. `provider` is touched on the main thread
    // only; the analyser also reads `disposed`.
    val camera = remember { CameraHold() }
    DisposableEffect(Unit) {
        onDispose {
            camera.disposed = true
            camera.provider?.unbindAll()
            camera.provider = null
            analysisExecutor.shutdown()
        }
    }
    AndroidView(
        modifier = Modifier.fillMaxSize(),
        factory = { ctx ->
            val view = PreviewView(ctx)
            val providerFuture = ProcessCameraProvider.getInstance(ctx)
            providerFuture.addListener({
                // The screen may have gone before the provider was ready.
                if (camera.disposed) return@addListener
                val provider = providerFuture.get()
                val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
                val analysis = ImageAnalysis.Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()
                analysis.setAnalyzer(analysisExecutor) { proxy ->
                    // read() leaves the frame open; it is closed here, always.
                    proxy.use { frame ->
                        if (delivered.get() || camera.disposed) return@use
                        // A QR reads at any rotation, so the frame goes in as it is.
                        val texts = runCatching { reader.read(frame) }.getOrDefault(emptyList()).map { it.text }
                        val code = firstPairingCode(texts)
                        if (code != null && delivered.compareAndSet(false, true)) {
                            ContextCompat.getMainExecutor(ctx).execute { onCode(code) }
                        }
                    }
                }
                provider.unbindAll()
                provider.bindToLifecycle(
                    lifecycleOwner,
                    CameraSelector.DEFAULT_BACK_CAMERA,
                    preview,
                    analysis,
                )
                camera.provider = provider
            }, ContextCompat.getMainExecutor(context))
            view
        },
    )
}

/** The camera this screen bound, so it can let go of it. */
private class CameraHold {
    @Volatile var disposed = false
    var provider: ProcessCameraProvider? = null
}
