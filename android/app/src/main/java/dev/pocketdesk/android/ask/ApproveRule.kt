package dev.pocketdesk.android.ask

import java.security.MessageDigest
import java.util.Base64
import org.json.JSONObject

/**
 * Approve with my phone: the rules for a request the computer's PAM helper
 * makes (sudo, polkit, the lock screen), kept apart from Android so they are
 * tested. The computer checks a Yes by its signature over [message], made
 * with this phone's Keystore key, against the key root pinned on the
 * computer; the Rust side (`core/src/protocol/approve.rs`) builds the same
 * bytes, and both tests pin one example.
 */
object ApproveRule {
    /** The key packet: this phone's public key, given or taken back. */
    const val KEY_PACKET = "kdeconnect.pocketdesk.approve_key"

    /** The first line of every signed message. */
    const val DOMAIN = "pocketdesk-approve-v1"

    data class Approval(
        val id: String,
        val kind: String,
        val service: String,
        val user: String,
        val command: String,
        val computer: String,
        val nonce: String,
        val issuedUnix: Long,
        val expiresUnix: Long,
    ) {
        /** What the request is for, as the computer said it. */
        val unlock: Boolean get() = kind == "unlock"
    }

    private val ID = Regex("^[A-Za-z0-9-]{1,64}$")
    private val NAME = Regex("^[A-Za-z0-9._][A-Za-z0-9._-]{0,31}$")
    private val HEX64 = Regex("^[0-9a-f]{64}$")

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
                command = o.optString("command", ""),
                computer = o.getString("computer"),
                nonce = o.getString("nonce"),
                issuedUnix = o.getLong("issued_unix"),
                expiresUnix = o.getLong("expires_unix"),
            )
            val ok = ID.matches(a.id) &&
                (a.kind == "admin" || a.kind == "unlock") &&
                NAME.matches(a.service) && NAME.matches(a.user) &&
                a.command.codePointCount(0, a.command.length) <= 300 && a.command.none { it.isISOControl() } &&
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
            a.command,
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

    /** What both apps show to compare a key: SHA-256's first 16 hex digits, in fours. */
    fun fingerprint(spki: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(spki).take(8)
            .joinToString("") { "%02x".format(it) }
            .chunked(4)
            .joinToString(" ")

    enum class Decision { Offer, Off, NoKey, Expired }

    /** Whether to offer an approval request: the switch on, a key here, in time. */
    fun decide(on: Boolean, hasKey: Boolean, expiresUnix: Long, nowUnix: Long): Decision = when {
        !on -> Decision.Off
        !hasKey -> Decision.NoKey
        nowUnix >= expiresUnix -> Decision.Expired
        else -> Decision.Offer
    }
}
