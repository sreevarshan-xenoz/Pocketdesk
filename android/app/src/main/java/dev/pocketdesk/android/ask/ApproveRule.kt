package dev.pocketdesk.android.ask

import java.security.MessageDigest
import java.util.Base64
import org.json.JSONArray
import org.json.JSONObject

/**
 * Approve with my phone: the rules for a request the computer's PAM helper
 * makes (sudo, polkit, the lock screen, or root pinning this phone's key),
 * kept apart from Android so they are tested. The computer checks a Yes by
 * its signature over [message], made with this phone's Keystore key,
 * against the key root pinned; the Rust side (`core/src/protocol/approve.rs`)
 * builds the same bytes, and both tests pin one example. Every text field
 * is printable ASCII with anything else written `⟨U+XXXX⟩` by the computer,
 * and is shown whole.
 */
object ApproveRule {
    /** The key packet: this phone's public key, given or taken back. */
    const val KEY_PACKET = "kdeconnect.pocketdesk.approve_key"

    /** The first line of every signed message. */
    const val DOMAIN = "pocketdesk-approve-v2"

    /** The service a pin request names. */
    const val PIN_SERVICE = "pocketdesk-pin"

    const val MAX_COMMAND = 4096
    const val MAX_ORIGIN = 512
    const val MAX_TTY = 64
    const val MAX_ACTION = 256

    data class Approval(
        val id: String,
        val kind: String,
        val service: String,
        val user: String,
        val requester: String,
        val action: String,
        val command: String,
        val tty: String,
        val origin: String,
        /** The pinned keys' SHA-256, lowercase hex, comma-separated. */
        val pinned: String,
        val computer: String,
        val nonce: String,
        val issuedUnix: Long,
        val expiresUnix: Long,
    ) {
        val unlock: Boolean get() = kind == "unlock"

        /** Root pinning this phone's key: the key is made now, attested. */
        val pin: Boolean get() = kind == "pin"
    }

    private val ID = Regex("^[A-Za-z0-9-]{1,64}$")
    private val NAME = Regex("^[A-Za-z0-9._][A-Za-z0-9._-]{0,31}$")
    private val HEX64 = Regex("^[0-9a-f]{64}$")
    private val ESCAPE = Regex("^⟨(U\\+[0-9A-F]{4,6}|SPx[1-9][0-9]{0,5})⟩")
    private const val SPACE_RUN = 3

    /** Whether [text] is what the computer's escaping makes: printable ASCII and `⟨U+XXXX⟩`. */
    fun isShownText(text: String, max: Int): Boolean {
        if (text.codePointCount(0, text.length) > max) return false
        // A run of spaces arrives as its count (SPx3000 in brackets), never as spaces.
        if (text.contains(" ".repeat(SPACE_RUN))) return false
        var i = 0
        while (i < text.length) {
            val c = text[i]
            if (c in ' '..'~') {
                i++
                continue
            }
            val m = ESCAPE.find(text.substring(i)) ?: return false
            if (m.value.contains("SPx") && m.value.filter { it.isDigit() }.toInt() < SPACE_RUN) return false
            i += m.value.length
        }
        return true
    }

    /** The approval the bridge kept with a question, or null when none or malformed. */
    fun parse(json: String?): Approval? {
        if (json.isNullOrBlank()) return null
        return runCatching {
            val o = JSONObject(json)
            val a = Approval(
                id = o.getString("id"),
                kind = o.getString("kind"),
                service = o.getString("service"),
                user = o.getString("user"),
                requester = o.optString("requester", ""),
                action = o.optString("action", ""),
                command = o.optString("command", ""),
                tty = o.optString("tty", ""),
                origin = o.optString("origin", ""),
                pinned = o.optString("pinned", ""),
                computer = o.getString("computer"),
                nonce = o.getString("nonce"),
                issuedUnix = o.getLong("issued_unix"),
                expiresUnix = o.getLong("expires_unix"),
            )
            val ok = ID.matches(a.id) &&
                (a.kind == "admin" || a.kind == "unlock" || a.kind == "pin") &&
                NAME.matches(a.service) && NAME.matches(a.user) &&
                (a.requester.isEmpty() || NAME.matches(a.requester)) &&
                (a.kind != "pin" || a.service == PIN_SERVICE) &&
                isShownText(a.action, MAX_ACTION) &&
                isShownText(a.command, MAX_COMMAND) &&
                isShownText(a.tty, MAX_TTY) &&
                isShownText(a.origin, MAX_ORIGIN) &&
                (a.pinned.isEmpty() || a.pinned.split(",").let { k -> k.size <= 8 && k.all { HEX64.matches(it) } }) &&
                HEX64.matches(a.computer) && HEX64.matches(a.nonce) &&
                a.expiresUnix > a.issuedUnix && a.expiresUnix - a.issuedUnix <= 60
            if (ok) a else null
        }.getOrNull()
    }

    /**
     * The bytes a Yes signs: [DOMAIN], then each field in a fixed order,
     * each `<length in UTF-8 bytes>:<value>\n`, then `yes`.
     */
    fun message(a: Approval): ByteArray {
        val fields = listOf(
            DOMAIN,
            a.id,
            a.kind,
            a.service,
            a.user,
            a.requester,
            a.action,
            a.command,
            a.tty,
            a.origin,
            a.pinned,
            a.computer,
            a.nonce,
            a.issuedUnix.toString(),
            a.expiresUnix.toString(),
            "yes",
        )
        val out = java.io.ByteArrayOutputStream()
        for (field in fields) {
            val bytes = field.toByteArray(Charsets.UTF_8)
            out.write("${bytes.size}:".toByteArray(Charsets.UTF_8))
            out.write(bytes)
            out.write('\n'.code)
        }
        return out.toByteArray()
    }

    /** A pin request's Key Attestation challenge: SHA-256 of its [message]. */
    fun pinChallenge(a: Approval): ByteArray = MessageDigest.getInstance("SHA-256").digest(message(a))

    /** The signed Yes's body. */
    fun signedAnswer(id: String, signature: ByteArray): String? =
        if (ID.matches(id)) {
            JSONObject()
                .put("id", id)
                .put("answer", "yes")
                .put("signature", Base64.getEncoder().encodeToString(signature))
                .toString()
        } else {
            null
        }

    /** A pin's Yes: the new key's attestation chain, leaf first. */
    fun pinAnswer(id: String, chain: List<ByteArray>): String? =
        if (ID.matches(id) && chain.isNotEmpty() && chain.size <= 8) {
            val certs = JSONArray()
            chain.forEach { certs.put(Base64.getEncoder().encodeToString(it)) }
            JSONObject().put("id", id).put("answer", "yes").put("attestation", certs).toString()
        } else {
            null
        }

    /** The key packet's body: the key ([spki], X.509 SubjectPublicKeyInfo), or null to take it back. */
    fun keyBody(spki: ByteArray?, biometricOnly: Boolean): String =
        if (spki == null) {
            JSONObject().put("public_key", JSONObject.NULL).toString()
        } else {
            JSONObject()
                .put("public_key", Base64.getEncoder().encodeToString(spki))
                .put("biometric_only", biometricOnly)
                .toString()
        }

    /** What both apps show to compare a key: its whole SHA-256, in fours. */
    fun fingerprint(spki: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(spki)
            .joinToString("") { "%02x".format(it) }
            .chunked(4)
            .joinToString(" ")

    /** The pinned keys a request names. */
    fun pinnedKeys(a: Approval): List<String> = if (a.pinned.isEmpty()) emptyList() else a.pinned.split(",")

    /**
     * The code root types at the pin for a key: the last eight hex digits
     * of its SHA-256, in two fours (review I5: it binds the pinned key to
     * this phone).
     */
    fun code(spki: ByteArray): String = fingerprint(spki).replace(" ", "").takeLast(8).chunked(4).joinToString(" ")

    enum class Decision { Offer, Off, NoKey, Expired }

    /**
     * Whether to offer an approval request: the switch on, in time, and a
     * key here (a pin request makes its own).
     */
    fun decide(on: Boolean, hasKey: Boolean, pin: Boolean, expiresUnix: Long, nowUnix: Long): Decision = when {
        !on -> Decision.Off
        !hasKey && !pin -> Decision.NoKey
        nowUnix >= expiresUnix -> Decision.Expired
        else -> Decision.Offer
    }
}
