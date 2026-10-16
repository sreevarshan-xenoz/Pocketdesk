package dev.pocketdesk.android.automation

/**
 * What happens, told to the phone's automation apps (ecosystem 1f): each
 * event is an Android broadcast `dev.pocketdesk.android.event.<KIND>` that
 * Tasker's or MacroDroid's "Intent Received" picks up. Any app on the phone
 * can receive one, so an event carries metadata only: the computer's saved
 * address, the kind, and the few fields [FIELDS] allows for it, never a
 * file's name, a link's address or a task's output.
 */
object PhoneEvents {
    const val ACTION_PREFIX = "dev.pocketdesk.android.event."
    const val EXTRA_PREFIX = "dev.pocketdesk.android.extra."
    /** The computer's address as `docs/phone-automation.md`'s intents take it. */
    const val EXTRA_ADDRESS = EXTRA_PREFIX + "ADDRESS"
    /** The kind in lowercase, as `pocketdesk events` names the computer's. */
    const val EXTRA_EVENT = EXTRA_PREFIX + "EVENT"

    enum class Kind {
        SESSION_STARTED,
        SESSION_ENDED,
        FILE_RECEIVED,
        FILES_SENT,
        LINK_RECEIVED,
        TASK_FINISHED,
        ;

        val action: String get() = ACTION_PREFIX + name
        val event: String get() = name.lowercase()
    }

    /** The fields each kind may carry; anything else is dropped. */
    val FIELDS: Map<Kind, Set<String>> = mapOf(
        Kind.SESSION_STARTED to setOf("mode"),
        Kind.SESSION_ENDED to setOf("reason"),
        Kind.FILE_RECEIVED to setOf("mime", "size"),
        Kind.FILES_SENT to setOf("count", "failed"),
        Kind.LINK_RECEIVED to emptySet(),
        Kind.TASK_FINISHED to setOf("task", "state", "code", "timed_out", "reason"),
    )

    /** One event, ready to broadcast. */
    data class Event(val kind: Kind, val extras: Map<String, Any>)

    /**
     * The event of [kind] for the computer at [address], with only the
     * [FIELDS] its kind allows (each under `EXTRA_PREFIX` plus the field in
     * upper case); `null` when the address is blank. Values are strings,
     * numbers or booleans; others are dropped.
     */
    fun build(kind: Kind, address: String, fields: Map<String, Any?> = emptyMap()): Event? {
        if (address.isBlank()) return null
        val allowed = FIELDS.getValue(kind)
        val extras = linkedMapOf<String, Any>(
            EXTRA_EVENT to kind.event,
            EXTRA_ADDRESS to address,
        )
        for ((key, value) in fields) {
            if (key !in allowed) continue
            when (value) {
                is String, is Int, is Long, is Boolean -> extras[EXTRA_PREFIX + key.uppercase()] = value
                else -> {}
            }
        }
        return Event(kind, extras)
    }
}
