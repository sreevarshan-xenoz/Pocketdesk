package dev.pocketdesk.android.windows

import org.json.JSONObject

/**
 * Phone apps tier 2 (separate windows): whether this phone opens an app in a
 * window of its own for the computer, and how many. Pure, for JVM tests;
 * [dev.pocketdesk.android.control.PhoneActions] and
 * [dev.pocketdesk.android.cast.CastService] do it.
 *
 * It is optional and off by default. It needs Shizuku installed and running
 * (the owner's own choice, never asked for here), Shizuku's permission for
 * this app (its own dialog, asked only when the owner turns the switch on),
 * and the switch.
 */
object WindowRule {
    /** The most windows at once: the computer's own cap, `core::streaming::cast::MAX_WINDOWS`. */
    const val MAX_WINDOWS = 4

    /** Android 10: the first with an app launched onto another display. */
    const val MIN_SDK = 29

    /** The sides and density a window may be asked at, as the computer's `WindowAsk`. */
    const val MIN_SIDE = 240
    const val MAX_SIDE = 3840
    const val MIN_DPI = 120
    const val MAX_DPI = 640

    /** Where separate windows stand on this phone. */
    enum class Mode {
        /** Android is older than [MIN_SDK]. */
        TOO_OLD,

        /** Shizuku is not running (or not installed): the row is not offered. */
        NO_SHIZUKU,

        /** Shizuku runs, and this app has not been allowed to use it. */
        NEEDS_PERMISSION,

        /** Everything is there but the owner's switch is off (the default). */
        OFF,

        /** Windows are offered to the phone's own computer. */
        ON,
    }

    /**
     * Where it stands: [sdk] Android's version, [shizukuRunning] whether
     * Shizuku's binder answers, [permitted] whether this app holds Shizuku's
     * permission, [switchOn] the owner's switch.
     */
    fun mode(sdk: Int, shizukuRunning: Boolean, permitted: Boolean, switchOn: Boolean): Mode = when {
        sdk < MIN_SDK -> Mode.TOO_OLD
        !shizukuRunning -> Mode.NO_SHIZUKU
        !switchOn -> Mode.OFF
        !permitted -> Mode.NEEDS_PERMISSION
        else -> Mode.ON
    }

    /**
     * Whether the Settings row is shown: only with Shizuku running (the row
     * is the place to turn it on), or while the switch is on, so the owner
     * can always turn it off.
     */
    fun rowShown(sdk: Int, shizukuRunning: Boolean, switchOn: Boolean): Boolean =
        sdk >= MIN_SDK && (shizukuRunning || switchOn)

    /** What an ask for a window comes to. */
    enum class Decision {
        /** Make the window. */
        START,

        /** The phone cannot make one now: the computer opens the app the ordinary way. */
        UNAVAILABLE,

        /** As many windows as it takes are open already, or that id is. */
        LIMIT,
    }

    /**
     * What to do with an ask for window [id] in [mode], with the windows in
     * [open] (their ids). Never more than [MAX_WINDOWS], and never an id
     * twice.
     */
    fun decide(mode: Mode, id: Int, open: Set<Int>): Decision = when {
        mode != Mode.ON -> Decision.UNAVAILABLE
        id in open || open.size >= MAX_WINDOWS -> Decision.LIMIT
        else -> Decision.START
    }

    /**
     * What the phone says of windows with its list of apps (to its own
     * computer only): `{on, max}`. Off unless [mode] is [Mode.ON].
     */
    fun listBody(mode: Mode): JSONObject = JSONObject().put("on", mode == Mode.ON).put("max", MAX_WINDOWS)

    /** An ask for a window: its id and the size and density it is made at. */
    data class Ask(val id: Int, val width: Int, val height: Int, val dpi: Int) {
        val sound: Boolean
            get() = id in 1..MAX_WINDOWS && width in MIN_SIDE..MAX_SIDE && height in MIN_SIDE..MAX_SIDE &&
                dpi in MIN_DPI..MAX_DPI
    }

    /** Where a control batch goes in a session. */
    enum class Target { WINDOWS, SCREEN, DROP }

    /**
     * Where a control batch goes: a batch for app windows to them; any other
     * to the phone's own screen only in a session that shows the screen and
     * only while its control is on. A session of windows alone has no screen
     * to control, so every other batch is dropped there.
     */
    fun target(windowsOnly: Boolean, forWindows: Boolean, screenControlOn: Boolean): Target = when {
        forWindows -> Target.WINDOWS
        windowsOnly || !screenControlOn -> Target.DROP
        else -> Target.SCREEN
    }

    /**
     * Whether an input event may be injected: [setDisplay] gives the event its
     * display (null when this Android has no such call) and [send] injects it
     * (null when there is no input service). Both must be there: without a
     * display id an event would land on the phone's own screen. `setDisplay`
     * returns nothing, so its result is never taken for an answer.
     */
    fun <E> inject(event: E, displayId: Int, setDisplay: ((E, Int) -> Unit)?, send: ((E) -> Boolean)?): Boolean {
        if (setDisplay == null || send == null) return false
        setDisplay(event, displayId)
        return send(event)
    }

    /** The shell's uid, and root's: Shizuku's own server calls with these. */
    const val UID_SHELL = 2000
    const val UID_ROOT = 0

    /**
     * Whether a call from [caller] is taken by the helper process: this app's
     * uid ([expected]; -1 while unknown, which takes none), and for the one
     * call Shizuku itself makes ([destroy]) its shell or root.
     */
    fun callerAllowed(caller: Int, expected: Int, destroy: Boolean): Boolean =
        (expected >= 0 && caller == expected) || (destroy && (caller == UID_SHELL || caller == UID_ROOT))

    /**
     * Whether a set of fingers is one the helper injects: [action] is Android's with the changing
     * finger's index in its high bits. A first DOWN or last UP has one finger; a POINTER_DOWN or
     * POINTER_UP has two to [WindowInput.MAX_POINTERS] and an index among them; a MOVE has any,
     * at index 0. Ids are distinct and below the cap, and every point is a number on a
     * [width] by [height] display (a point past an edge is held to it, see [clampPixel]).
     */
    fun pointersOk(action: Int, ids: IntArray, xs: FloatArray, ys: FloatArray, width: Int, height: Int): Boolean {
        val count = ids.size
        if (count !in 1..WindowInput.MAX_POINTERS || xs.size != count || ys.size != count) return false
        if (width < 1 || height < 1) return false
        if (ids.any { it !in 0 until WindowInput.MAX_POINTERS } || ids.toSet().size != count) return false
        if (!xs.all { it.isFinite() } || !ys.all { it.isFinite() }) return false
        val index = action shr WindowInput.ACTION_POINTER_INDEX_SHIFT
        return when (action and WindowInput.ACTION_MASK) {
            WindowInput.ACTION_DOWN, WindowInput.ACTION_UP -> count == 1 && index == 0
            WindowInput.ACTION_MOVE -> index == 0
            WindowInput.ACTION_POINTER_DOWN, WindowInput.ACTION_POINTER_UP -> count >= 2 && index in 0 until count
            else -> false
        }
    }

    /** A point's coordinate held to a display [size] pixels on that side. */
    fun clampPixel(v: Float, size: Int): Float = v.coerceIn(0f, (size - 1).coerceAtLeast(0).toFloat())

    /** The only keys the helper presses: Back, Enter and Delete. */
    fun keyAllowed(code: Int): Boolean =
        code == WindowInput.KEYCODE_BACK || code == WindowInput.KEYCODE_ENTER || code == WindowInput.KEYCODE_DEL

    /** Apps never shown in a window: the ones that change what the phone or this helper may do. */
    private val DENIED = setOf(
        "dev.pocketdesk.android",
        "com.android.settings",
        "com.android.settings.intelligence",
        "com.android.providers.settings",
        "com.android.systemui",
        "com.android.shell",
        "com.android.packageinstaller",
        "com.google.android.packageinstaller",
        "com.android.permissioncontroller",
        "com.google.android.permissioncontroller",
        "com.android.managedprovisioning",
        "com.android.vending",
        "com.android.documentsui",
        "moe.shizuku.privileged.api",
        "rikka.shizuku",
        "com.coloros.safecenter",
        "com.oplus.battery",
        "com.samsung.android.settings",
        "com.miui.securitycenter",
    )

    /**
     * Whether [pkg] may not be opened in a window: it is on the fixed list,
     * is [own] (Pocketdesk), or is one of [resolved] (the packages the
     * phone's own settings, application-management, install and permission
     * screens resolve to on this phone). Anything else a phone maker ships
     * under another name is not caught: SECURITY limit 83 says so.
     */
    fun denied(pkg: String, own: String, resolved: Set<String>): Boolean =
        pkg == own || pkg in DENIED || pkg in resolved

    // DisplayManager's VIRTUAL_DISPLAY_FLAG_* (some are hidden), as scrcpy's own display uses them.
    private const val OWN_CONTENT_ONLY = 1 shl 3
    private const val SUPPORTS_TOUCH = 1 shl 6
    private const val ROTATES_WITH_CONTENT = 1 shl 7
    private const val DESTROY_CONTENT_ON_REMOVAL = 1 shl 8
    private const val TRUSTED = 1 shl 10
    private const val OWN_DISPLAY_GROUP = 1 shl 11
    private const val ALWAYS_UNLOCKED = 1 shl 12
    private const val TOUCH_FEEDBACK_DISABLED = 1 shl 13
    private const val OWN_FOCUS = 1 shl 14
    private const val DEVICE_DISPLAY_GROUP = 1 shl 15

    /**
     * The flags an app window's display is made with at Android [sdk]: not public and not a
     * presentation, so no other app can put anything on it; only the shell that made it launches
     * there. (Whether `am start --display` from the shell may launch other apps on such a display
     * before Android 13 is an owed device check.) Android 13 is Tiramisu, 14 Upside Down Cake.
     */
    fun displayFlags(sdk: Int): Int {
        var flags = OWN_CONTENT_ONLY or SUPPORTS_TOUCH or ROTATES_WITH_CONTENT or
            DESTROY_CONTENT_ON_REMOVAL
        if (sdk >= 33) flags = flags or TRUSTED or OWN_DISPLAY_GROUP or ALWAYS_UNLOCKED or TOUCH_FEEDBACK_DISABLED
        if (sdk >= 34) flags = flags or OWN_FOCUS or DEVICE_DISPLAY_GROUP
        return flags
    }

    private val COMPONENT = Regex("""[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+/[A-Za-z0-9_.$]+""")

    /**
     * Whether [component] is "package/class" and nothing a shell would read as
     * more: it is passed to `am start -n`, so only a package name, a slash and
     * a class name are let through.
     */
    fun componentOk(component: String): Boolean = COMPONENT.matches(component)

    /** The ask in an open's `window` object, or null when there is none or it is not sound. */
    fun parseAsk(json: JSONObject?): Ask? {
        json ?: return null
        val ask = Ask(json.optInt("id", 0), json.optInt("width", 0), json.optInt("height", 0), json.optInt("dpi", 0))
        return ask.takeIf { it.sound }
    }
}
