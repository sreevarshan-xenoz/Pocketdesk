package dev.pocketdesk.android.status

import dev.pocketdesk.android.R

/**
 * Remote revoke: the devices paired with a computer, as it answered this
 * phone, and what a removal there came to. The computer answers only a
 * phone paired with its owner's own code; it never names a key or an
 * address. Kept free of Android and org.json so the JVM tests reach it.
 */
object DevicesThereRule {
    /** One device paired with the computer. */
    data class Device(
        val id: String,
        val name: String,
        val computer: Boolean,
        val owner: Boolean,
        val guest: Boolean,
        val connected: Boolean,
        val you: Boolean,
    )

    /** How a removal went: removed, or why not as a string resource. */
    data class Outcome(val removed: Boolean, val reason: Int?)

    /** A device as shown: its name, else the start of its id. */
    fun shownName(device: Device): String =
        device.name.trim().ifEmpty { device.id.take(8) }

    /** Whether the sheet offers Remove for it: never for this phone itself. */
    fun removable(device: Device): Boolean = !device.you

    /** This phone first, then the owner's devices, then the rest, each by name. */
    fun sorted(devices: List<Device>): List<Device> =
        devices.sortedWith(
            compareBy<Device>({ !it.you }, { !it.owner }, { shownName(it).lowercase() }),
        )

    /** The words beside a device's name, as string resources. */
    fun kinds(device: Device): List<Int> = buildList {
        add(if (device.computer) R.string.there_kind_computer else R.string.there_kind_phone)
        if (device.you) {
            add(R.string.there_kind_you)
        } else if (device.owner) {
            add(R.string.there_kind_yours)
        }
        if (device.guest) add(R.string.there_kind_guest)
        if (device.connected) add(R.string.there_kind_connected)
    }

    /** What the owner reads for a computer's refusal code. */
    fun refusal(code: String?): Int = when (code) {
        "not_owner" -> R.string.there_refused_not_owner
        "too_many" -> R.string.there_refused_too_many
        "yourself" -> R.string.there_refused_yourself
        "not_paired" -> R.string.there_refused_not_paired
        "last_owner" -> R.string.there_refused_last_owner
        else -> R.string.there_refused_failed
    }

    /** A removal's answer: `removed`, else its `reason`. */
    fun outcome(removed: Boolean, reason: String?): Outcome =
        if (removed) Outcome(true, null) else Outcome(false, refusal(reason))

    /** The list once [id] was removed there. */
    fun without(devices: List<Device>, id: String): List<Device> = devices.filterNot { it.id == id }
}
