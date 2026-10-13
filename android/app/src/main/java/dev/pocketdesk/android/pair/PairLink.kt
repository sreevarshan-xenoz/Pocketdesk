package dev.pocketdesk.android.pair

import java.nio.charset.StandardCharsets
import java.util.Base64

/**
 * A desktop's pairing code: everything needed to find, trust and pair with
 * it, as the Pocketdesk desktop app encodes it (`core/src/local/pair_link.rs`
 * is the other half; both are held to one fixture URI by their tests).
 *
 * `pocketdesk://pair?v=1&name&id&a=ip,ip&cp&sp&pin&fp&ep` — addresses best
 * first, `fp` the hex SHA-256 of the desktop's TLS certificate, `ep` its iroh
 * endpoint JSON in base64url. Values are percent-encoded.
 */
data class PairLink(
    val name: String,
    val deviceId: String,
    val addresses: List<String>,
    val controlPort: Int,
    val streamPort: Int,
    val pin: String,
    val fingerprint: String?,
    val endpoint: String?,
) {
    companion object {
        private const val PREFIX = "pocketdesk://pair?"
        private const val DEFAULT_CONTROL_PORT = 1716
        private const val DEFAULT_STREAM_PORT = 4716
        private const val MAX_NAME = 32

        /**
         * A desktop name fit to show: the link chooses it, so control and
         * formatting characters (a right-to-left override could make one name
         * read as another) are dropped and it is cut to a sane length.
         */
        fun displayName(raw: String): String {
            val clean = raw.filter { c ->
                val type = Character.getType(c)
                type != Character.CONTROL.toInt() &&
                    type != Character.FORMAT.toInt() &&
                    type != Character.UNASSIGNED.toInt() &&
                    type != Character.PRIVATE_USE.toInt()
            }.trim()
            return if (clean.length > MAX_NAME) clean.take(MAX_NAME) + "…" else clean
        }

        fun parse(uri: String): Result<PairLink> = runCatching {
            val query = uri.trim().removePrefix(PREFIX)
            require(query != uri.trim()) { "not a Pocketdesk pairing link" }
            val fields = query.split('&').filter { it.isNotEmpty() }.associate { pair ->
                val key = pair.substringBefore('=')
                key to percentDecode(pair.substringAfter('=', ""))
            }
            require(fields["v"] == "1") { "unsupported pairing link version" }
            val pin = fields["pin"].orEmpty()
            require(pin.length == 6 && pin.all { it in '0'..'9' }) { "the PIN must be 6 digits" }
            val addresses = fields["a"].orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }
            require(addresses.isNotEmpty()) { "the link names no address" }
            fun port(key: String, default: Int): Int =
                fields[key]?.let { v -> v.toIntOrNull()?.takeIf { it in 1..65535 } ?: error("bad port '$key'") }
                    ?: default
            PairLink(
                name = fields["name"].orEmpty(),
                deviceId = fields["id"].orEmpty(),
                addresses = addresses,
                controlPort = port("cp", DEFAULT_CONTROL_PORT),
                streamPort = port("sp", DEFAULT_STREAM_PORT),
                pin = pin,
                fingerprint = fields["fp"]?.takeIf { it.isNotEmpty() }?.also { fp ->
                    require(fp.length == 64 && fp.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) {
                        "the certificate fingerprint must be 64 hex digits"
                    }
                },
                endpoint = fields["ep"]?.let { encoded ->
                    String(Base64.getUrlDecoder().decode(encoded), StandardCharsets.UTF_8)
                },
            )
        }

        private fun percentDecode(value: String): String {
            val out = java.io.ByteArrayOutputStream()
            var i = 0
            while (i < value.length) {
                when (val c = value[i]) {
                    '%' -> {
                        require(i + 3 <= value.length) { "bad percent-encoding" }
                        out.write(value.substring(i + 1, i + 3).toInt(16))
                        i += 3
                    }
                    '+' -> {
                        out.write(' '.code)
                        i++
                    }
                    else -> {
                        out.write(c.toString().toByteArray(StandardCharsets.UTF_8))
                        i++
                    }
                }
            }
            return out.toString(StandardCharsets.UTF_8.name())
        }
    }
}

/** The first Pocketdesk pairing code among the codes a camera frame holds. */
fun firstPairingCode(values: List<String?>): String? =
    values.firstOrNull { it?.startsWith("pocketdesk://pair?") == true }
