package com.indicatrixsuite.mobile

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.app.NativeActivity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.print.PrintAttributes
import android.print.PrintManager
import android.provider.DocumentsContract
import android.util.Log
import android.view.WindowManager
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.annotation.RequiresApi
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

/**
 * The app's only activity: a [NativeActivity] that runs the Rust/Slint application and
 * provides the Android services it cannot reach itself.
 *
 * Rust calls the public methods below by name over JNI (`platform_android/mod.rs`), and
 * answers to asynchronous requests come back through the `native*` functions (declared
 * at the bottom, implemented in `platform_android/natives.rs`). Keep both sides in step.
 *
 * Convention: every service method returns `null` on success and a sentence for the
 * user on failure. `thermalHeadroom`, `cacheDirPath`, `deviceFacts` and `lastExitInfo`
 * return their value.
 *
 * NativeActivity is not a ComponentActivity, so results come through
 * `startActivityForResult` / `onActivityResult` with request codes mapped to the Rust
 * request ids.
 */
@Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
class MainActivity : NativeActivity() {

    /** A system dialog we started and are waiting for. UI thread only. */
    private class PendingRequest(
        val rustId: Long,
        val kind: Int,
        val saveBytes: ByteArray? = null,
    )

    private val requests = HashMap<Int, PendingRequest>()
    private var nextRequestCode = FIRST_REQUEST_CODE

    /** The WebView of the print job in flight; the print adapter needs it alive. */
    private var printView: WebView? = null
    private var backNavigation: BackNavigationController? = null

    /** The browse for the paired computer; null until Rust asks for one. UI thread only. */
    private var discovery: NsdDiscovery? = null

    /** The system dialog for the local network is open. UI thread only. */
    private var askingLocalNetwork = false

    // ---------------------------------------------------------------------------------
    // Lifecycle
    // ---------------------------------------------------------------------------------

    override fun onCreate(savedInstanceState: Bundle?) {
        // Edge to edge: Slint reads the insets and keeps the UI out of the system bars.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        super.onCreate(savedInstanceState)
        cleanMailCache()
        if (Build.VERSION.SDK_INT >= 33) {
            backNavigation = BackNavigationController(this, this::nativeBackCommitted, this::nativeBackProgress)
        }
        // A recreated activity gets the same launch intent again; import it only once.
        if (savedInstanceState == null) {
            handleIncoming(intent)
        }
        // Rust keeps this activity to start and stop the scanner and the rediscovery.
        attached = this
        nativeAttachActivity()
    }

    override fun onStop() {
        // Browsing for the computer is only useful while the app is on screen.
        discovery?.stop()
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncoming(intent)
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    override fun onDestroy() {
        discovery?.stop()
        discovery = null
        if (askingLocalNetwork) {
            // The dialog dies with the activity: Rust must not wait for its answer.
            askingLocalNetwork = false
            nativeLocalNetworkAnswered(0)
        }
        // Rust lets go of this activity, unless a newer one was attached in the meantime.
        if (attached === this) {
            attached = null
            nativeDetachActivity()
        }
        backNavigation?.close()
        backNavigation = null
        printView = null
        super.onDestroy()
    }

    // ---------------------------------------------------------------------------------
    // Services Rust calls
    // ---------------------------------------------------------------------------------
    // Every method in this section is called only from Rust over JNI (`platform_android`);
    // Kotlin sees no callers, so never remove one as unused. JNI calls arrive on a
    // non-UI thread: UI work hops to the UI thread through [onUi].

    private val vault by lazy { RemoteCredentialVault(this) }

    /** `openDocuments(JIZ)Ljava/lang/String;`: starts the system picker for [kind]. */
    fun openDocuments(requestId: Long, kind: Int, allowMultiple: Boolean): String? = guarded {
        val intent = when (kind) {
            KIND_DESIGN_FOLDER -> Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            KIND_DESIGNS, KIND_HDR_MAP -> Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*" // design and .hdr files have no reliable MIME type
                putExtra(Intent.EXTRA_ALLOW_MULTIPLE, allowMultiple && kind == KIND_DESIGNS)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            else -> throw IllegalArgumentException("Unknown request kind $kind.")
        }
        startForResult(intent, requestId, kind, null)
        null
    }

    /**
     * `createDocument(JLjava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;`: save dialog.
     * The bytes wait in the pending request until the user picks a file.
     */
    fun createDocument(requestId: Long, name: String, mime: String, bytes: ByteArray): String? = guarded {
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = mime.ifBlank { "application/octet-stream" }
            putExtra(Intent.EXTRA_TITLE, name)
        }
        startForResult(intent, requestId, KIND_SAVE, bytes)
        null
    }

    /**
     * `share(Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;`: share sheet for a
     * copy written to cache/share/.
     */
    fun share(name: String, mime: String, bytes: ByteArray): String? = guarded {
        val dir = File(cacheDir, "share")
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Could not prepare the share folder.")
        val file = File(dir, safeFileName(name))
        dir.listFiles()?.forEach { if (it != file) it.delete() }
        file.writeBytes(bytes)
        val uri = FileProvider.getUriForFile(this, FILE_AUTHORITY, file)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = mime.ifBlank { "application/octet-stream" }
            putExtra(Intent.EXTRA_STREAM, uri)
            clipData = ClipData.newRawUri(file.name, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val chooser = Intent.createChooser(send, null).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        onUi { startActivity(chooser) }
        null
    }

    /**
     * `composeEmail(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;`:
     * opens the mail app with one zip attached, for the logs of the developer page. The bytes
     * go to cache/mail/ (a FileProvider root of its own: every [share] empties cache/share/).
     * The intent is `ACTION_SEND` with a `mailto:` selector, so only mail apps answer; where
     * none does, the plain chooser opens. `resolveActivity` is never called: the manifest
     * has no `<queries>`, so it would report nothing on Android 11 and later.
     */
    fun composeEmail(to: String, subject: String, body: String, name: String, bytes: ByteArray): String? = guarded {
        val dir = File(cacheDir, "mail")
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Could not prepare the mail folder.")
        val file = File(dir, safeFileName(name))
        file.writeBytes(bytes)
        val uri = FileProvider.getUriForFile(this, FILE_AUTHORITY, file)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "application/zip"
            putExtra(Intent.EXTRA_EMAIL, arrayOf(to))
            putExtra(Intent.EXTRA_SUBJECT, subject)
            putExtra(Intent.EXTRA_TEXT, body)
            putExtra(Intent.EXTRA_STREAM, uri)
            clipData = ClipData.newRawUri(file.name, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val mailOnly = Intent(send).apply { selector = Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:")) }
        try {
            onUi { startActivity(mailOnly) }
        } catch (e: ActivityNotFoundException) {
            val chooser = Intent.createChooser(send, null).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            onUi { startActivity(chooser) }
        }
        null
    }

    /**
     * `printHtml(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;`: loads [html] in a
     * WebView, then opens the print dialog.
     */
    fun printHtml(title: String, html: String): String? = guarded {
        val jobName = title.ifBlank { "Pleochroma" }
        onUi {
            val view = WebView(this)
            view.settings.javaScriptEnabled = false
            view.settings.allowFileAccess = false
            view.settings.allowContentAccess = false
            var started = false
            view.webViewClient = object : WebViewClient() {
                override fun onPageFinished(page: WebView, url: String?) {
                    if (started) return
                    started = true
                    try {
                        val manager = getSystemService(PrintManager::class.java)
                        manager.print(jobName, page.createPrintDocumentAdapter(jobName), PrintAttributes.Builder().build())
                    } catch (e: Exception) {
                        Log.w(TAG, "starting the print job failed", e)
                    }
                }
            }
            printView = view // the print adapter needs the view alive
            view.loadDataWithBaseURL(null, html, "text/html", "utf-8", null)
        }
        null
    }

    /**
     * `setClipboardText(Ljava/lang/String;)Ljava/lang/String;`: replaces the primary clip with
     * plain text.
     */
    fun setClipboardText(text: String): String? = guarded {
        onUi {
            val clipboard = getSystemService(ClipboardManager::class.java)
            clipboard.setPrimaryClip(ClipData.newPlainText("Pleochroma", text))
        }
        null
    }

    /**
     * `keepScreenOn(Z)Ljava/lang/String;`: sets or clears the keep-screen-on window flag.
     */
    fun keepScreenOn(on: Boolean): String? = guarded {
        onUi {
            if (on) {
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
        null
    }

    /**
     * `thermalHeadroom()F`: the current thermal headroom, or NaN where the device does not
     * report it (before API 30).
     */
    fun thermalHeadroom(): Float =
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                getSystemService(PowerManager::class.java).getThermalHeadroom(0)
            } else {
                Float.NaN
            }
        } catch (e: Exception) {
            Log.w(TAG, "thermal headroom is unavailable", e)
            Float.NaN
        }

    /** `openUrl(Ljava/lang/String;)Ljava/lang/String;`: opens a web link in the browser. */
    fun openUrl(url: String): String? = guarded {
        val uri = Uri.parse(url)
        if (uri.scheme != "https" && uri.scheme != "http") {
            throw IllegalArgumentException("Only web links can be opened.")
        }
        onUi { startActivity(Intent(Intent.ACTION_VIEW, uri)) }
        null
    }

    /** `cacheDirPath()Ljava/lang/String;`: the app's cache directory. */
    fun cacheDirPath(): String = cacheDir.absolutePath

    /**
     * `deviceFacts()Ljava/lang/String;`: what the diagnostics may say about the device, as
     * `key=value` lines. No account, no `ANDROID_ID`, no serial number.
     */
    fun deviceFacts(): String {
        val version = try {
            packageManager.getPackageInfo(packageName, 0).versionName ?: ""
        } catch (e: Exception) {
            ""
        }
        val soc = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) Build.SOC_MODEL ?: "" else ""
        val facts = listOf(
            "app_version" to version,
            "android_release" to (Build.VERSION.RELEASE ?: ""),
            "sdk_int" to Build.VERSION.SDK_INT.toString(),
            "manufacturer" to (Build.MANUFACTURER ?: ""),
            "model" to (Build.MODEL ?: ""),
            "soc_model" to soc,
            "abi" to (Build.SUPPORTED_ABIS.firstOrNull() ?: ""),
            "total_ram_mb" to totalRamMb().toString(),
            "fingerprint" to (Build.FINGERPRINT ?: ""),
        )
        return facts.joinToString("\n") { (key, value) -> "$key=${oneLine(value)}" }
    }

    /** The device's total memory in MB, or 0 when the system does not say. */
    private fun totalRamMb(): Long =
        try {
            val info = ActivityManager.MemoryInfo()
            getSystemService(ActivityManager::class.java).getMemoryInfo(info)
            info.totalMem / BYTES_PER_MB
        } catch (e: Exception) {
            0L
        }

    /**
     * `lastExitInfo(J)Ljava/lang/String;`: the newest process exit of this app that is later
     * than [afterMs], as `key=value` lines (`reason`, `description`, `timestamp_ms`, `pid`,
     * `trace_file`), or `null` when there is none or the system has no exit history (before
     * Android 11). The system's trace stream is saved into files/logs/ here: an ANR trace as
     * text (`anr-<stamp>.txt`), a native crash tombstone whole and binary
     * (`crash-snapshot-<stamp>.pb`). Rust passes the time of the last exit it reported, so
     * an exit is reported once.
     */
    fun lastExitInfo(afterMs: Long): String? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return null
        return try {
            readLastExit(afterMs)
        } catch (e: Exception) {
            Log.w(TAG, "reading the last process exit failed", e)
            null
        }
    }

    @RequiresApi(Build.VERSION_CODES.R)
    private fun readLastExit(afterMs: Long): String? {
        val manager = getSystemService(ActivityManager::class.java)
        val exit = manager.getHistoricalProcessExitReasons(packageName, 0, 1).firstOrNull() ?: return null
        if (exit.timestamp <= afterMs) return null
        val reason = when (exit.reason) {
            ApplicationExitInfo.REASON_CRASH_NATIVE -> "crash_native"
            ApplicationExitInfo.REASON_CRASH -> "crash"
            ApplicationExitInfo.REASON_ANR -> "anr"
            ApplicationExitInfo.REASON_USER_REQUESTED -> "user_requested"
            ApplicationExitInfo.REASON_LOW_MEMORY -> "low_memory"
            else -> "other"
        }
        val lines = arrayListOf(
            "reason=$reason",
            "description=${oneLine(exit.description ?: "")}",
            "timestamp_ms=${exit.timestamp}",
            "pid=${exit.pid}",
        )
        saveExitTrace(exit, reason)?.let { lines.add("trace_file=$it") }
        return lines.joinToString("\n")
    }

    /**
     * Saves the exit's trace stream into files/logs/ and returns the file name, or `null` when
     * there is none. An ANR trace is text and is cut at [ANR_TRACE_CAP]; a native crash
     * tombstone is a binary protobuf and is kept whole or not at all (a cut one cannot be read).
     */
    @RequiresApi(Build.VERSION_CODES.R)
    private fun saveExitTrace(exit: ApplicationExitInfo, reason: String): String? {
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date(exit.timestamp))
        val name: String
        val cap: Int
        val whole: Boolean
        when (reason) {
            "anr" -> {
                name = "anr-$stamp.txt"; cap = ANR_TRACE_CAP; whole = false
            }
            "crash_native" -> {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null
                name = "crash-snapshot-$stamp.pb"; cap = SNAPSHOT_CAP; whole = true
            }
            else -> return null
        }
        val stream = exit.traceInputStream ?: return null
        val out = ByteArrayOutputStream()
        stream.use {
            val buffer = ByteArray(8192)
            var total = 0
            while (total <= cap) {
                val count = it.read(buffer, 0, minOf(buffer.size, cap + 1 - total))
                if (count < 0) break
                out.write(buffer, 0, count)
                total += count
            }
        }
        var bytes = out.toByteArray()
        if (bytes.size > cap) {
            if (whole) return null
            bytes = bytes.copyOf(cap)
        }
        if (bytes.isEmpty()) return null
        val dir = File(filesDir, "logs")
        if (!dir.isDirectory && !dir.mkdirs()) return null
        File(dir, name).writeBytes(bytes)
        return name
    }

    /** One line: line breaks become spaces, so a `key=value` list stays parseable. */
    private fun oneLine(text: String): String = text.replace('\n', ' ').replace('\r', ' ')

    /** Deletes the mail attachments of earlier sessions (older than [MAIL_MAX_AGE_MS]). */
    private fun cleanMailCache() {
        thread(name = "pleochroma-mail-clean") {
            try {
                val cutoff = System.currentTimeMillis() - MAIL_MAX_AGE_MS
                File(cacheDir, "mail").listFiles()?.forEach { if (it.lastModified() < cutoff) it.delete() }
            } catch (e: Exception) {
                Log.w(TAG, "cleaning the mail folder failed", e)
            }
        }
    }

    /** `systemPowerSave()Z`: whether the battery saver is on. */
    fun systemPowerSave(): Boolean =
        try {
            getSystemService(PowerManager::class.java).isPowerSaveMode
        } catch (e: Exception) {
            false
        }

    /** `networkMetered()I`: 1 metered, 0 unmetered, -1 unknown. */
    fun networkMetered(): Int =
        try {
            val manager = getSystemService(ConnectivityManager::class.java)
            when {
                manager.activeNetwork == null -> -1
                manager.isActiveNetworkMetered -> 1
                else -> 0
            }
        } catch (e: Exception) {
            -1
        }

    /** `setBackTarget(II)V`: mirrors Rust's Back state into the [BackNavigationController]. */
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    fun setBackTarget(target: Int, version: Int) {
        runOnUiThread { backNavigation?.setTarget(target, version) }
    }

    /** `storeRemoteCredentials(Ljava/lang/String;)Ljava/lang/String;`: Keystore-wrapped store. */
    fun storeRemoteCredentials(bundle: String): String? = guarded {
        vault.store(bundle)
        null
    }

    /**
     * `loadRemoteCredentials()Ljava/lang/String;`: the bundle, or `null` if none. An exception
     * escapes when the bundle cannot be unlocked.
     */
    fun loadRemoteCredentials(): String? = vault.load()

    /**
     * `forgetRemoteCredentials()Ljava/lang/String;`: deletes the encrypted file and the
     * Keystore key.
     */
    fun forgetRemoteCredentials(): String? = guarded {
        vault.forget()
        null
    }

    /**
     * `startQrScan()Ljava/lang/String;`: opens the QR scanner ([ScannerActivity]). The camera
     * permission is asked there, only now. A scanned `pleochroma://pair` link comes back in
     * [onActivityResult] and goes to Rust through [nativePairingLink], like a deep link.
     */
    fun startQrScan(): String? = guarded {
        onUi { startActivityForResult(Intent(this, ScannerActivity::class.java), SCAN_REQUEST_CODE) }
        null
    }

    /**
     * `startRediscovery()Ljava/lang/String;`: browses `_pleochroma._tcp` for 30 seconds
     * (multicast lock held only meanwhile). Each service found is reported to
     * [nativeServiceFound]; the end of the browse, by whatever cause, to
     * [nativeRediscoveryEnded].
     */
    fun startRediscovery(): String? = guarded {
        onUi {
            val browser = discovery ?: NsdDiscovery(
                this,
                { name, version, ca, addresses -> nativeServiceFound(name, version, ca, addresses) },
                { nativeRediscoveryEnded() },
            ).also { discovery = it }
            browser.start()
        }
        null
    }

    /**
     * `ensureLocalNetwork()I`: Android 17 (API 37) blocks every connection to a device on the
     * local network until the user allows `ACCESS_LOCAL_NETWORK`. Returns 1 when the app may
     * use it already (or the Android version asks for nothing), 0 when the system dialog is
     * open (the answer comes back through [nativeLocalNetworkAnswered]; a second call while
     * it is open also returns 0 and shares that answer), and -1 when the request could not
     * be started (Rust then tries the connection anyway).
     */
    fun ensureLocalNetwork(): Int =
        try {
            onUi {
                when {
                    Build.VERSION.SDK_INT < LOCAL_NETWORK_SDK -> 1
                    checkSelfPermission(PERMISSION_LOCAL_NETWORK) == PackageManager.PERMISSION_GRANTED -> 1
                    askingLocalNetwork -> 0
                    else -> {
                        askingLocalNetwork = true
                        try {
                            requestPermissions(arrayOf(PERMISSION_LOCAL_NETWORK), LOCAL_NETWORK_REQUEST_CODE)
                        } catch (e: Exception) {
                            // No dialog is open, so nothing will answer: do not wait for one.
                            askingLocalNetwork = false
                            throw e
                        }
                        0
                    }
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "asking for local network access failed", e)
            -1
        }

    /** `stopRediscovery()Ljava/lang/String;`: ends the browse and releases the multicast lock. */
    fun stopRediscovery(): String? = guarded {
        onUi { discovery?.stop() }
        null
    }

    /** Starts [intent] on the UI thread under a new request code tied to a [PendingRequest]. */
    private fun startForResult(intent: Intent, rustId: Long, kind: Int, saveBytes: ByteArray?) {
        onUi {
            val code = nextRequestCode++
            requests[code] = PendingRequest(rustId, kind, saveBytes)
            try {
                startActivityForResult(intent, code)
            } catch (e: Exception) {
                requests.remove(code)
                throw e
            }
        }
    }

    /** Runs [block] on the UI thread, waits up to [UI_WAIT_MS] and rethrows what it threw. */
    private fun <R> onUi(block: () -> R): R {
        val result = AtomicReference<Any?>()
        val failure = AtomicReference<Throwable?>()
        val done = CountDownLatch(1)
        runOnUiThread {
            try {
                result.set(block())
            } catch (t: Throwable) {
                failure.set(t)
            } finally {
                done.countDown()
            }
        }
        if (!done.await(UI_WAIT_MS, TimeUnit.MILLISECONDS)) {
            throw IOException("The app did not respond in time.")
        }
        failure.get()?.let { throw it }
        @Suppress("UNCHECKED_CAST")
        return result.get() as R
    }

    /** A file name safe inside cache/share/: no separators, no leading dots. */
    private fun safeFileName(name: String): String {
        val base = name.substringAfterLast('/').substringAfterLast('\\')
        val cleaned = base.replace(Regex("[^A-Za-z0-9._ -]"), "_").trimStart('.').trim().take(100)
        return cleaned.ifEmpty { "export" }
    }

    private external fun nativeBackCommitted(target: Int, version: Int)
    private external fun nativeBackProgress(target: Int, version: Int, progress: Float)

    // ---------------------------------------------------------------------------------
    // System dialog results
    // ---------------------------------------------------------------------------------

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == SCAN_REQUEST_CODE) {
            val link = data?.getStringExtra(ScannerActivity.EXTRA_LINK)
            // The scanner already checked the prefix; Rust reads the link and asks the user.
            if (resultCode == RESULT_OK && link != null) nativePairingLink(link)
            return
        }
        val request = requests.remove(requestCode) ?: return
        if (request.kind == KIND_SAVE) {
            finishSave(request, resultCode, data)
        } else {
            finishPick(request, resultCode, data)
        }
    }

    /**
     * The answer of the local network dialog ([ensureLocalNetwork]). An empty result means the
     * dialog was interrupted: that counts as a no, and the next attempt asks again.
     */
    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != LOCAL_NETWORK_REQUEST_CODE) return
        askingLocalNetwork = false
        val granted = grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED
        nativeLocalNetworkAnswered(if (granted) 1 else 0)
    }

    /** Reads the picked files on a background thread and hands them to Rust. */
    private fun finishPick(request: PendingRequest, resultCode: Int, data: Intent?) {
        val id = request.rustId
        if (resultCode != RESULT_OK || data == null) {
            nativeRequestDone(id, null) // canceled: no files
            return
        }
        thread(name = "pleochroma-pick") {
            val error = try {
                val reader = DocumentReader(contentResolver)
                val budget = ReadBudget()
                if (request.kind == KIND_DESIGN_FOLDER) {
                    data.data?.let { deliverTree(id, reader, budget, it) }
                } else {
                    for (uri in urisOf(data)) {
                        deliver(id, reader, budget, uri, reader.displayName(uri))
                    }
                }
                null
            } catch (e: Exception) {
                Log.w(TAG, "reading the picked files failed", e)
                describe(e)
            }
            nativeRequestDone(id, error)
        }
    }

    /** Writes the bytes of a save request to the chosen document. */
    private fun finishSave(request: PendingRequest, resultCode: Int, data: Intent?) {
        val id = request.rustId
        val uri = data?.data
        val bytes = request.saveBytes
        if (resultCode != RESULT_OK || uri == null || bytes == null) {
            nativeSaveDone(id, null, null) // cancelled
            return
        }
        thread(name = "pleochroma-save") {
            try {
                val stream = contentResolver.openOutputStream(uri, "wt")
                    ?: throw IOException("Could not open the file for writing.")
                stream.use { it.write(bytes) }
                nativeSaveDone(id, DocumentReader(contentResolver).displayName(uri), null)
            } catch (e: Exception) {
                Log.w(TAG, "saving the document failed", e)
                nativeSaveDone(id, null, describe(e))
            }
        }
    }

    private fun deliverTree(id: Long, reader: DocumentReader, budget: ReadBudget, tree: Uri) {
        for (entry in reader.listTree(tree)) {
            val uri = DocumentsContract.buildDocumentUriUsingTree(tree, entry.documentId)
            deliver(id, reader, budget, uri, entry.name)
        }
    }

    private fun deliver(id: Long, reader: DocumentReader, budget: ReadBudget, uri: Uri, name: String) {
        nativeFileReady(id, name, reader.read(uri, name, budget))
    }

    /** Every URI in a picker result: the single `data` and each `clipData` item. */
    private fun urisOf(intent: Intent): List<Uri> {
        val uris = LinkedHashSet<Uri>()
        intent.data?.let { uris.add(it) }
        val clip = intent.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) {
                clip.getItemAt(i).uri?.let { uris.add(it) }
            }
        }
        return uris.toList()
    }

    // ---------------------------------------------------------------------------------
    // "Open with" and "Share to"
    // ---------------------------------------------------------------------------------

    /**
     * Reads the files another app sent (VIEW, SEND, SEND_MULTIPLE) and delivers them to
     * Rust as one batch on request id 0. A file that cannot be read is skipped.
     */
    private fun handleIncoming(intent: Intent?) {
        if (intent == null) return
        if (handlePairingLink(intent)) return
        val uris: List<Uri> = when (intent.action) {
            Intent.ACTION_VIEW -> listOfNotNull(intent.data)
            Intent.ACTION_SEND -> listOfNotNull(streamExtra(intent))
            Intent.ACTION_SEND_MULTIPLE -> streamListExtra(intent)
            else -> emptyList()
        }
        if (uris.isEmpty()) return
        thread(name = "pleochroma-incoming") {
            val reader = DocumentReader(contentResolver)
            val budget = ReadBudget()
            for (uri in uris) {
                try {
                    deliver(INCOMING_ID, reader, budget, uri, reader.displayName(uri))
                } catch (e: Exception) {
                    Log.w(TAG, "an incoming file could not be read", e)
                }
            }
            nativeRequestDone(INCOMING_ID, null)
        }
    }

    /**
     * A `pleochroma://pair?...` link: hands it to Rust, which opens the confirm sheet. The link
     * carries a one-time pairing code, so it is never logged, and the intent that held it is
     * replaced afterwards so a recreated activity cannot deliver it a second time.
     * Returns whether [intent] was a pairing link.
     */
    private fun handlePairingLink(intent: Intent): Boolean {
        val uri = intent.data
        if (intent.action != Intent.ACTION_VIEW || uri == null ||
            !"pleochroma".equals(uri.scheme, ignoreCase = true) ||
            !"pair".equals(uri.host, ignoreCase = true)
        ) {
            return false
        }
        val link = uri.toString()
        setIntent(Intent(Intent.ACTION_MAIN))
        nativePairingLink(link)
        return true
    }

    private fun streamExtra(intent: Intent): Uri? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }

    private fun streamListExtra(intent: Intent): List<Uri> {
        val list = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
        }
        return list?.toList() ?: emptyList()
    }

    // ---------------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------------

    private fun guarded(action: () -> String?): String? =
        try {
            action()
        } catch (e: Exception) {
            Log.w(TAG, "a platform service failed", e)
            describe(e)
        }

    /** A sentence for the user for [e]. */
    private fun describe(e: Exception): String =
        when {
            e is ActivityNotFoundException -> "No app is available to do this."
            e is ImportLimitException -> e.message ?: "The file is too large."
            e.message.isNullOrBlank() -> e.javaClass.simpleName
            else -> e.message ?: e.javaClass.simpleName
        }

    // ---------------------------------------------------------------------------------
    // Rust (platform_android/natives.rs)
    // ---------------------------------------------------------------------------------

    private external fun nativeFileReady(requestId: Long, name: String, bytes: ByteArray)

    private external fun nativeRequestDone(requestId: Long, error: String?)

    private external fun nativeSaveDone(requestId: Long, displayName: String?, error: String?)

    /** A pairing link (`pleochroma://pair?...`); implemented in `src/remote.rs`, not in natives.rs. */
    private external fun nativePairingLink(link: String)

    /** Gives Rust this activity; implemented in `src/remote.rs`. */
    private external fun nativeAttachActivity()

    /**
     * One service found while browsing: its name, the TXT values `v` and `ca`, and its IPv4
     * addresses as a comma-separated `ip:port` list; implemented in `src/remote.rs`.
     */
    private external fun nativeServiceFound(name: String, version: String, ca: String, addresses: String)

    /**
     * The browse ended (its time ran out, the app stopped, it could not start, or Rust asked
     * for it to stop); implemented in `src/remote.rs`. Called on the UI thread: Rust only
     * clears its session and never calls back into this class from here.
     */
    private external fun nativeRediscoveryEnded()

    /**
     * The answer of the local network dialog: 1 allowed, 0 refused or interrupted;
     * implemented in `src/remote.rs`. Called on the UI thread: Rust only records the answer
     * and queues the work for its own thread.
     */
    private external fun nativeLocalNetworkAnswered(granted: Int)

    /** Tells Rust that this activity is gone; implemented in `src/remote.rs`. */
    private external fun nativeDetachActivity()

    companion object {
        private const val TAG = "pleochroma"

        /** The activity Rust currently holds (singleTask: normally the only one). UI thread. */
        private var attached: MainActivity? = null

        /** Request kinds sent from Rust (`KIND_*` in platform_android/mod.rs). */
        private const val KIND_DESIGNS = 0
        private const val KIND_HDR_MAP = 1
        private const val KIND_DESIGN_FOLDER = 2

        /** The FileProvider authority declared in the manifest. */
        private const val FILE_AUTHORITY = "com.indicatrixsuite.mobile.files"

        /** Request kind for a save dialog (never sent from Rust). */
        private const val KIND_SAVE = 100

        /** Rust request id for files the user did not pick (opened or shared into the app). */
        private const val INCOMING_ID = 0L

        private const val FIRST_REQUEST_CODE = 0x1000

        /** The request code of the QR scanner; outside the range of [FIRST_REQUEST_CODE]. */
        private const val SCAN_REQUEST_CODE = 0x0F00

        /** The request code of the local network dialog (permission codes stay below 0x10000). */
        private const val LOCAL_NETWORK_REQUEST_CODE = 0x0F01

        /**
         * The permission to reach devices on the local network, a runtime permission from
         * Android 17 (API 37) on. A string, so this compiles against any compile SDK.
         */
        private const val PERMISSION_LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
        private const val LOCAL_NETWORK_SDK = 37
        private const val UI_WAIT_MS = 2000L

        /** Caps of the saved exit traces: an ANR text, a whole native crash tombstone. */
        private const val ANR_TRACE_CAP = 256 * 1024
        private const val SNAPSHOT_CAP = 2 * 1024 * 1024

        /** A mail attachment older than this is deleted at the next start. */
        private const val MAIL_MAX_AGE_MS = 60L * 60L * 1000L
        private const val BYTES_PER_MB = 1024L * 1024L

        init {
            System.loadLibrary("pleochroma_mobile_android")
        }
    }
}
