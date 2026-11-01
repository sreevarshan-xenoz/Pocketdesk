package dev.pocketdesk.android.control

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Build
import android.util.Base64
import androidx.core.graphics.createBitmap
import org.json.JSONArray
import org.json.JSONObject

/**
 * The apps this phone lists for its own computer, with their icons (Apps
 * part 1). The list comes from the launcher's own query (the manifest's
 * `<queries>` entry for the launcher intent, never QUERY_ALL_PACKAGES) and
 * carries each app's label, package and version code. An icon is sent once:
 * the computer says which it keeps (`have`, package to version), and only
 * the others go, in batches after the list. Pure where it can be, for JVM
 * tests; [collect] and [icon] read the phone.
 */
object AppsList {
    /** The icons' side, in pixels: a grid cell on the computer is smaller. */
    const val ICON_PX = 64

    /** The most icons in a packet, as the computer takes at most. */
    const val MAX_ICONS_PER_PACKET = 40

    /** The most base64 characters in an icons packet: well under the 4 MiB a line may be. */
    const val MAX_PACKET_CHARS = 400_000

    /** One app: its package, its label and its version code. */
    data class App(val pkg: String, val label: String, val version: Long)

    /** One icon, a PNG as base64, for [pkg] at [version]. */
    data class Icon(val pkg: String, val version: Long, val png: String)

    /** The apps whose icon the computer does not have at their version. */
    fun missing(apps: List<App>, have: Map<String, Long>): List<App> =
        apps.filter { have[it.pkg] != it.version }

    /** [items] in order, cut into batches of at most [maxCount] and [maxWeight] in all by [weight]. */
    fun <T> batches(items: List<T>, weight: (T) -> Int, maxCount: Int, maxWeight: Int): List<List<T>> {
        val out = mutableListOf<List<T>>()
        var batch = mutableListOf<T>()
        var held = 0
        for (item in items) {
            val w = weight(item)
            if (batch.isNotEmpty() && (batch.size >= maxCount || held + w > maxWeight)) {
                out += batch
                batch = mutableListOf()
                held = 0
            }
            batch += item
            held += w
        }
        if (batch.isNotEmpty()) out += batch
        return out
    }

    /** The list packet's body: `{apps: [{id, name, v}]}`. */
    fun listBody(apps: List<App>): JSONObject {
        val list = JSONArray()
        apps.forEach { list.put(JSONObject().put("id", it.pkg).put("name", it.label).put("v", it.version)) }
        return JSONObject().put("apps", list)
    }

    /** An icons packet's body: `{icons: [{id, v, png}]}`. */
    fun iconsBody(icons: List<Icon>): JSONObject {
        val list = JSONArray()
        icons.forEach { list.put(JSONObject().put("id", it.pkg).put("v", it.version).put("png", it.png)) }
        return JSONObject().put("icons", list)
    }

    /**
     * The apps that open from the launcher, by label, never [self] (this
     * app), each package once, at most [max].
     */
    fun collect(context: Context, max: Int): List<App> {
        val pm = context.packageManager
        val launcher = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        val seen = mutableSetOf<String>()
        return runCatching { pm.queryIntentActivities(launcher, 0) }.getOrDefault(emptyList())
            .mapNotNull { info ->
                val pkg = info.activityInfo?.packageName ?: return@mapNotNull null
                if (pkg == context.packageName || !seen.add(pkg)) return@mapNotNull null
                App(
                    pkg,
                    runCatching { info.loadLabel(pm).toString() }.getOrDefault(pkg),
                    version(pm, pkg),
                )
            }
            .sortedBy { it.label.lowercase() }
            .take(max)
    }

    @Suppress("DEPRECATION")
    private fun version(pm: PackageManager, pkg: String): Long =
        runCatching {
            val info = pm.getPackageInfo(pkg, 0)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) info.longVersionCode else info.versionCode.toLong()
        }.getOrDefault(0L)

    /** [app]'s icon as a [ICON_PX] PNG in base64, or null when it cannot be drawn. */
    fun icon(context: Context, app: App): Icon? =
        runCatching {
            val drawable = context.packageManager.getApplicationIcon(app.pkg)
            val bitmap = createBitmap(ICON_PX, ICON_PX)
            drawable.setBounds(0, 0, ICON_PX, ICON_PX)
            drawable.draw(Canvas(bitmap))
            val out = java.io.ByteArrayOutputStream()
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            bitmap.recycle()
            Icon(app.pkg, app.version, Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP))
        }.getOrNull()
}
