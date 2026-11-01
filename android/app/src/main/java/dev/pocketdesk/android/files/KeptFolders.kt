package dev.pocketdesk.android.files

import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.BatteryManager
import android.util.Log
import androidx.core.content.edit
import androidx.core.net.toUri
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import dev.pocketdesk.android.HostStore
import dev.pocketdesk.android.clip.ClipboardPushRule
import dev.pocketdesk.android.notify.PhoneLink
import dev.pocketdesk.android.service.TransferService
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.SecureRandom
import java.util.concurrent.TimeUnit

/**
 * "Folders kept on my computer": folders the owner picked on the phone, each
 * mirrored one way to the computer on a schedule, through the transfer path
 * the rest of the app uses (resume, whole-file SHA-256, the transfer
 * service's notice). Each folder is picked with Android's folder picker and
 * kept by its persisted read grant: no storage-wide access, and the owner
 * is asked once per folder.
 *
 * What was sent is remembered per folder (its size and modified time), so a
 * run sends only what is new or changed. A WorkManager job with its
 * constraints says when a run is due; the run itself is the transfer
 * service's, started here when Android lets a background app start it, else
 * by the notification listener's tick or when the app is opened.
 */
object KeptFolders {
    private const val TAG = "KeptFolders"
    private const val PREFS = "pocketdesk-kept-folders"
    private const val KEY_FOLDERS = "folders"
    private const val KEY_MIRROR = "mirror"
    private const val KEY_MODE = "mode"
    private const val KEY_HOURS = "hours"
    private const val KEY_COMPUTER = "computer"
    private const val KEY_DUE = "due"
    private const val WORK = "kept-folders"

    /** A kept folder: its id on the wire (32 hex digits, its own for good), its tree, and its name. */
    data class Folder(val id: String, val uri: String, val label: String)

    /** How a folder's last run went. [held] says why deletions were held back, if they were. */
    data class Status(
        val atMs: Long,
        val sent: Int,
        val failed: Int,
        val note: String?,
        val held: KeptRule.Held? = null,
    )

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** The kept folders, in the order they were added. */
    fun folders(ctx: Context): List<Folder> =
        runCatching {
            val array = JSONArray(prefs(ctx).getString(KEY_FOLDERS, "[]"))
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Folder(o.getString("id"), o.getString("uri"), o.getString("label"))
            }
        }.getOrDefault(emptyList()).filter { KeptRule.validId(it.id) }

    private fun save(ctx: Context, folders: List<Folder>) {
        val array = JSONArray()
        folders.forEach { array.put(JSONObject().put("id", it.id).put("uri", it.uri).put("label", it.label)) }
        prefs(ctx).edit { putString(KEY_FOLDERS, array.toString()) }
    }

    /** Whether Android still gives this app the folder: taken back in its settings, or its storage removed, it does not. */
    fun readable(ctx: Context, folder: Folder): Boolean =
        runCatching {
            ctx.contentResolver.persistedUriPermissions.any { it.isReadPermission && it.uri.toString() == folder.uri }
        }.getOrDefault(false)

    /**
     * Keep the folder the picker returned, with its read grant for good;
     * false when Android keeps none. The same folder twice is one.
     */
    fun add(ctx: Context, tree: Uri, label: String): Boolean {
        runCatching { ctx.contentResolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
            .onFailure { return false }
        val all = folders(ctx)
        if (all.none { it.uri == tree.toString() }) {
            save(ctx, all + Folder(newId(), tree.toString(), label))
        }
        schedule(ctx)
        return true
    }

    /**
     * Stop keeping [folder]: what was remembered of it goes, and its grant
     * is given back unless another setting still reads the same folder (the
     * grants are per folder, not per feature). The computer's copy stays.
     */
    fun remove(ctx: Context, folder: Folder) {
        save(ctx, folders(ctx).filterNot { it.id == folder.id })
        recordFile(ctx, folder.id).delete()
        prefs(ctx).edit {
            remove("status_${folder.id}")
            remove("computer_${folder.id}")
        }
        val others =
            folders(ctx).any { it.uri == folder.uri } ||
                PhoneFolders.all(ctx).any { it.uri == folder.uri } ||
                dev.pocketdesk.android.Prefs.downloadTree(ctx) == folder.uri
        if (!others) {
            runCatching {
                ctx.contentResolver.releasePersistableUriPermission(folder.uri.toUri(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        }
        schedule(ctx)
    }

    private fun newId(): String {
        val bytes = ByteArray(16).also { SecureRandom().nextBytes(it) }
        return bytes.joinToString("") { "%02x".format(it) }
    }

    /** "Mirror deletions": off by default; on, a file deleted on the phone is deleted on the computer too, if the computer allows it. */
    fun mirror(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_MIRROR, false)

    fun setMirror(ctx: Context, on: Boolean) = prefs(ctx).edit { putBoolean(KEY_MIRROR, on) }

    fun mode(ctx: Context): KeptRule.Mode =
        runCatching { KeptRule.Mode.valueOf(prefs(ctx).getString(KEY_MODE, null).orEmpty()) }
            .getOrDefault(KeptRule.Mode.WIFI_CHARGING)

    fun hours(ctx: Context): Int = prefs(ctx).getInt(KEY_HOURS, KeptRule.DEFAULT_HOURS)

    fun setSchedule(ctx: Context, mode: KeptRule.Mode, hours: Int) {
        prefs(ctx).edit {
            putString(KEY_MODE, mode.name)
            putInt(KEY_HOURS, hours)
        }
        schedule(ctx)
    }

    /** The address of the computer the folders go to; empty for the phone's own computer. */
    fun computer(ctx: Context): String = prefs(ctx).getString(KEY_COMPUTER, "").orEmpty()

    fun setComputer(ctx: Context, address: String) = prefs(ctx).edit { putString(KEY_COMPUTER, address) }

    /** The computer the folders go to: the one picked if it is still saved, else the phone's own. */
    fun target(ctx: Context): HostStore.Host? {
        val chosen = computer(ctx)
        return HostStore.hosts(ctx).firstOrNull { it.address == chosen } ?: PhoneLink.target(ctx)
    }

    fun status(ctx: Context, id: String): Status? =
        runCatching {
            val o = JSONObject(prefs(ctx).getString("status_$id", null) ?: return null)
            Status(
                o.getLong("at"),
                o.getInt("sent"),
                o.getInt("failed"),
                o.optString("note").ifEmpty { null },
                o.optString("held").takeIf { it.isNotEmpty() }?.let { runCatching { KeptRule.Held.valueOf(it) }.getOrNull() },
            )
        }.getOrNull()

    fun setStatus(ctx: Context, id: String, status: Status) {
        val o = JSONObject().put("at", status.atMs).put("sent", status.sent).put("failed", status.failed)
        status.note?.let { o.put("note", it) }
        status.held?.let { o.put("held", it.name) }
        prefs(ctx).edit { putString("status_$id", o.toString()) }
    }

    // What was sent, per folder: a text file of marks (KeptRule.write), beside which the
    // computer it was sent to is remembered, so another computer starts from nothing.

    private fun recordFile(ctx: Context, id: String) = File(File(ctx.filesDir, "kept"), "$id.tsv")

    /** What was sent of [folder] to the computer [computerId]; nothing when another computer was sent to. */
    fun record(ctx: Context, folder: Folder, computerId: String): Map<String, KeptRule.Mark> {
        if (prefs(ctx).getString("computer_${folder.id}", null) != computerId) return emptyMap()
        val file = recordFile(ctx, folder.id)
        return runCatching { if (file.isFile) KeptRule.read(file.readText()) else emptyMap() }.getOrDefault(emptyMap())
    }

    /** Keep [record] as what was sent of [folder] to [computerId]; written whole and moved into place. */
    fun saveRecord(ctx: Context, folder: Folder, computerId: String, record: Map<String, KeptRule.Mark>) {
        val file = recordFile(ctx, folder.id)
        runCatching {
            file.parentFile?.mkdirs()
            val temp = File(file.parentFile, "${folder.id}.tmp")
            temp.writeText(KeptRule.write(record))
            if (!temp.renameTo(file)) {
                file.delete()
                check(temp.renameTo(file)) { "could not keep the record" }
            }
            prefs(ctx).edit { putString("computer_${folder.id}", computerId) }
        }.onFailure { Log.w(TAG, "could not keep what was sent of a folder", it) }
    }

    /** How many files and bytes a [record] holds, for the folder's summary. */
    fun summary(record: Map<String, KeptRule.Mark>): Pair<Int, Long> = record.size to record.values.sumOf { it.size }

    // The schedule.

    /** Make the job match the folders and the schedule: none kept, no job. */
    fun schedule(ctx: Context) {
        runCatching {
            val work = WorkManager.getInstance(ctx)
            if (folders(ctx).isEmpty()) {
                work.cancelUniqueWork(WORK)
                return
            }
            val needs = KeptRule.needs(mode(ctx))
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(if (needs.unmetered) NetworkType.UNMETERED else NetworkType.CONNECTED)
                .setRequiresCharging(needs.charging)
                .build()
            val period = KeptRule.periodHours(mode(ctx), hours(ctx)).toLong()
            val request = PeriodicWorkRequestBuilder<KeptFoldersWorker>(period, TimeUnit.HOURS)
                .setConstraints(constraints)
                .build()
            work.enqueueUniquePeriodicWork(WORK, ExistingPeriodicWorkPolicy.UPDATE, request)
        }.onFailure { Log.w(TAG, "could not schedule the kept folders", it) }
    }

    @Volatile private var scheduled = false

    /** Schedule once per process: the job outlives it, and each change of folders or schedule does it again. */
    fun ensureScheduled(ctx: Context) {
        if (scheduled) return
        scheduled = true
        schedule(ctx)
    }

    fun markDue(ctx: Context) = prefs(ctx).edit { putBoolean(KEY_DUE, true) }

    private fun clearDue(ctx: Context) = prefs(ctx).edit { putBoolean(KEY_DUE, false) }

    fun due(ctx: Context): Boolean = prefs(ctx).getBoolean(KEY_DUE, false)

    /**
     * Start a run now if one is due, the schedule's conditions hold, and no
     * transfer runs: the job calls it, and so do the notification listener's
     * tick and the app's start, for a phone where Android would not let the
     * job start the service itself. A run that could not start stays due.
     */
    fun startIfDue(ctx: Context) {
        if (!due(ctx)) return
        if (folders(ctx).isEmpty()) {
            clearDue(ctx)
            return
        }
        if (TransferService.busy()) return
        if (!KeptRule.mayStart(mode(ctx), onWifi(ctx), charging(ctx))) return
        if (!start(ctx)) return
        clearDue(ctx)
    }

    /** Start a run now, whatever the schedule says (the owner asked); whether it was started. */
    fun start(ctx: Context): Boolean {
        val own = target(ctx) ?: return false
        return runCatching { TransferService.keep(ctx, own.address, own.controlPort) }
            .onFailure { Log.w(TAG, "the kept folders could not start now: ${it.message}") }
            .isSuccess
    }

    private fun onWifi(ctx: Context): Boolean {
        val cm = ctx.getSystemService(ConnectivityManager::class.java) ?: return false
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return ClipboardPushRule.onWifi(
            wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
            ethernet = caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET),
            notMetered = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED),
            vpn = caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN),
        )
    }

    private fun charging(ctx: Context): Boolean =
        ctx.getSystemService(BatteryManager::class.java)?.isCharging == true
}
