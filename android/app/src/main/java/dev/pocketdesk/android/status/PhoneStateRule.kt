package dev.pocketdesk.android.status

/**
 * What the phone tells its computer of itself beside its battery (L9): its
 * free storage, its network's kind and bars, its ringer. Kept apart from
 * Android so it is tested; [readPhoneState] reads it.
 */
object PhoneStateRule {
    const val WIFI = "wifi"
    const val MOBILE = "mobile"
    const val ETHERNET = "ethernet"
    const val NONE = "none"

    const val NORMAL = "normal"
    const val VIBRATE = "vibrate"
    const val SILENT = "silent"

    /** Free storage is said to this step, so it does not change with every file. */
    const val STORAGE_STEP_MB = 100L

    /** Decimal, as the phone's own Settings counts storage. */
    private const val MB = 1_000_000L

    /** Each part null when it could not be read. */
    data class State(
        val storageFreeMb: Long? = null,
        val storageTotalMb: Long? = null,
        val network: String? = null,
        /** 0–4 bars. */
        val signal: Int? = null,
        val ringer: String? = null,
    )

    /** Free and total storage in whole (decimal) MB, the free rounded down to [STORAGE_STEP_MB]; null for a reading that makes no sense. */
    fun storage(freeBytes: Long, totalBytes: Long): Pair<Long, Long>? {
        if (totalBytes <= 0 || freeBytes < 0 || freeBytes > totalBytes) return null
        val free = freeBytes / MB / STORAGE_STEP_MB * STORAGE_STEP_MB
        return free to totalBytes / MB
    }

    /** Wi-Fi's bars, 0–4, for its [rssi] in dBm; null for none known. */
    fun wifiBars(rssi: Int): Int? = when {
        rssi > 0 || rssi < -127 -> null
        rssi >= -55 -> 4
        rssi >= -66 -> 3
        rssi >= -77 -> 2
        rssi >= -88 -> 1
        else -> 0
    }

    /**
     * A modem's level as bars, or null: 0 is Android's "none or unknown"
     * (L9 review), so it is not said as no bars.
     */
    fun mobileBars(level: Int): Int? = level.takeIf { it in 1..4 }

    /**
     * Whether a change from [last] to [now] is sent at once: the network's
     * kind or the ringer. Storage and bars wait for the battery's resend.
     */
    fun changedNow(last: State, now: State): Boolean =
        last.network != now.network || last.ringer != now.ringer

    /** The state's fields for the battery packet's body, each with a comma before it; empty for none. */
    fun fields(state: State): String = buildString {
        state.storageFreeMb?.let { append(",\"storageFreeMb\":$it") }
        state.storageTotalMb?.let { append(",\"storageTotalMb\":$it") }
        state.network?.let { append(",\"network\":\"$it\"") }
        state.signal?.let { append(",\"signal\":$it") }
        state.ringer?.let { append(",\"ringer\":\"$it\"") }
    }
}
