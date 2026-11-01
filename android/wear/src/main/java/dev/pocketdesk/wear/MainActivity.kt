package dev.pocketdesk.wear

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import dev.pocketdesk.watchlink.WatchBodies
import dev.pocketdesk.watchlink.WatchButton
import dev.pocketdesk.watchlink.WatchClient
import dev.pocketdesk.watchlink.WatchPairing
import dev.pocketdesk.watchlink.WatchWire
import java.io.IOException
import java.util.Timer
import java.util.TimerTask

/**
 * The short list: the pairing with the phone, and then the buttons the phone
 * has on, the deck's buttons and the questions waiting. Plain views (no
 * Compose, no Wear Compose: a tile and a list need neither, and the APK
 * stays small). Everything goes through the phone over Bluetooth.
 */
class MainActivity : Activity() {
    private val main = Handler(Looper.getMainLooper())
    private lateinit var root: LinearLayout

    /** The line under the buttons: how the last tap went. */
    private var said: String? = null

    /** Bumped on each draw: an answer for an older one is dropped. */
    private var generation = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            // Room for a round screen's corners.
            val side = dp(20)
            setPadding(side, dp(36), side, dp(48))
        }
        setContentView(ScrollView(this).apply { addView(root) })
    }

    override fun onResume() {
        super.onResume()
        render()
    }

    override fun onPause() {
        generation++
        super.onPause()
    }

    @Deprecated("The plain Activity's callback; the permission is asked once.")
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        @Suppress("DEPRECATION")
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        render()
    }

    private fun render() {
        generation++
        root.removeAllViews()
        title(getString(R.string.main_title))
        when {
            !PhoneLink.hasPermission(this) -> {
                body(getString(R.string.main_need_permission))
                button(getString(R.string.main_allow)) {
                    requestPermissions(arrayOf(Manifest.permission.BLUETOOTH_CONNECT), PERMISSION)
                }
            }
            PhoneStore.paired(this) == null -> pairScreen()
            else -> listScreen(null)
        }
    }

    // ---- Pairing

    @SuppressLint("MissingPermission")
    private fun pairScreen() {
        body(getString(R.string.main_pair_title), bold = true)
        body(getString(R.string.main_pair_steps))
        val adapter = getSystemService(BluetoothManager::class.java)?.adapter
        val bonded = runCatching { adapter?.bondedDevices.orEmpty().toList() }.getOrDefault(emptyList())
        if (bonded.isEmpty()) {
            body(getString(R.string.main_no_phone))
            return
        }
        // The devices this watch is already paired with: no scan, no location.
        for (device in bonded) {
            button(device.name ?: device.address.takeLast(5)) { startPairing(device) }
        }
        said?.let { body(it) }
    }

    @SuppressLint("MissingPermission")
    private fun startPairing(device: BluetoothDevice) {
        val code = WatchPairing.newCode()
        generation++
        val mine = generation
        root.removeAllViews()
        title(getString(R.string.main_title))
        body(getString(R.string.main_code, code.chunked(3).joinToString(" ")), bold = true)
        body(getString(R.string.main_waiting))
        Thread({
            val result = runCatching { pair(device, code) }
            main.post {
                if (mine != generation) return@post
                result.onSuccess { key ->
                    PhoneStore.save(this, key, device.address)
                    said = null
                }.onFailure {
                    said = getString(R.string.main_pair_failed, it.message ?: "")
                }
                render()
            }
        }, "pocketdesk-pair").start()
    }

    /** One connection to [device] that proves [code]; the key the phone hands over. */
    @SuppressLint("MissingPermission")
    private fun pair(device: BluetoothDevice, code: String): ByteArray {
        val socket = device.createRfcommSocketToServiceRecord(WatchWire.SERVICE)
        // The phone holds the connection while its owner types the code: two minutes, then the socket is closed under the read.
        val timer = Timer("pocketdesk-pair-timeout", true)
        timer.schedule(object : TimerTask() {
            override fun run() {
                runCatching { socket.close() }
            }
        }, PAIR_MS)
        try {
            getSystemService(BluetoothManager::class.java)?.adapter?.cancelDiscovery()
            socket.connect()
            val client = WatchClient(socket.inputStream, socket.outputStream, null)
            client.open()
            return client.pair(code, Build.MODEL ?: "watch")
        } catch (e: IOException) {
            throw IOException(e.message ?: "no answer")
        } finally {
            timer.cancel()
            runCatching { socket.close() }
        }
    }

    // ---- The short list

    /** Draw the list from a run: the buttons the phone has on, the deck, the questions. */
    private fun listScreen(run: Run?) {
        val mine = generation
        val look = run?.look?.getOrNull()
        if (run == null) {
            body(getString(R.string.main_loading))
            Thread({
                val result = Phone.run(this, null, withDeck = true)
                main.post { if (mine == generation) { root.removeAllViews(); title(getString(R.string.main_title)); listScreen(result) } }
            }, "pocketdesk-list").start()
            return
        }
        if (look == null) {
            body(LineText.of(this, Phone.failed(run.look.exceptionOrNull())).orEmpty())
            button(getString(R.string.main_refresh)) { render() }
            forgetButton()
            return
        }
        look.computer.takeIf { it.isNotBlank() }?.let { body(it, dim = true) }
        said?.let { body(it) }
        fun tap(act: TilePlan.Act) {
            val now = ++generation
            root.removeAllViews()
            title(getString(R.string.main_title))
            body(getString(R.string.main_loading))
            Thread({
                val result = Phone.run(this, act, withDeck = true)
                main.post {
                    if (now != generation) return@post
                    said = LineText.of(this, result.line)
                    root.removeAllViews()
                    title(getString(R.string.main_title))
                    listScreen(result)
                }
            }, "pocketdesk-tap").start()
        }
        if (look.buttons.isEmpty()) body(getString(R.string.tile_nothing_on))
        if (WatchButton.Wake in look.buttons) button(getString(R.string.main_wake)) { tap(TilePlan.Act.Wake) }
        if (WatchButton.Ring in look.buttons) button(getString(R.string.main_ring)) { tap(TilePlan.Act.Ring) }
        if (WatchButton.Music in look.buttons) {
            button(getString(R.string.main_play)) { tap(TilePlan.Act.Music(dev.pocketdesk.watchlink.MusicCommand.PlayPause)) }
            button(getString(R.string.main_next)) { tap(TilePlan.Act.Music(dev.pocketdesk.watchlink.MusicCommand.Next)) }
            button(getString(R.string.main_previous)) { tap(TilePlan.Act.Music(dev.pocketdesk.watchlink.MusicCommand.Previous)) }
        }
        if (run.deck.isNotEmpty()) {
            body(getString(R.string.main_deck_title), bold = true)
            for (d in run.deck) button(d.label) { tap(TilePlan.Act.Deck(d.index, d.label)) }
        }
        if (look.asks.isNotEmpty()) {
            body(getString(R.string.main_asks_title), bold = true)
            for (ask in look.asks) {
                body(ask.text)
                if (ask.approval) body(getString(R.string.main_ask_on_phone), dim = true)
                button(getString(R.string.main_ask_no)) { tap(TilePlan.Act.Deny(ask.id)) }
            }
        }
        button(getString(R.string.main_refresh)) { said = null; render() }
        forgetButton()
    }

    private fun forgetButton() = button(getString(R.string.main_forget)) {
        PhoneStore.forget(this)
        said = null
        render()
    }

    // ---- Views

    private fun dp(n: Int): Int = (n * resources.displayMetrics.density).toInt()

    private fun title(text: String) = body(text, bold = true, size = 16f)

    private fun body(text: String, bold: Boolean = false, dim: Boolean = false, size: Float = 13f) {
        root.addView(
            TextView(this).apply {
                this.text = text
                setTextSize(TypedValue.COMPLEX_UNIT_SP, size)
                gravity = Gravity.CENTER_HORIZONTAL
                if (bold) setTypeface(typeface, android.graphics.Typeface.BOLD)
                if (dim) alpha = 0.7f
                setPadding(0, dp(4), 0, dp(4))
            },
        )
    }

    private fun button(text: String, onClick: () -> Unit) {
        root.addView(
            Button(this).apply {
                this.text = text
                isAllCaps = false
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
                setOnClickListener { _: View -> onClick() }
            },
            LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT).apply {
                topMargin = dp(4)
            },
        )
    }

    private companion object {
        const val PERMISSION = 1
        const val PAIR_MS = 130_000L
    }
}
