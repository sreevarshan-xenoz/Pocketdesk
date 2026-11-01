package dev.pocketdesk.android.ask

import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.util.Base64
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ApproveRuleTest {
    private val computer = "ab".repeat(32)
    private val nonce = "01".repeat(32)

    private fun json(
        id: String = "a-1",
        kind: String = "admin",
        service: String = "sudo",
        user: String = "alice",
        requester: String = "alice",
        command: String = "/usr/bin/sudo pacman -Syu",
        tty: String = "/dev/pts/3",
        origin: String = "sudo(812) < bash(700) < kitty(650)",
        computer: String = this.computer,
        nonce: String = this.nonce,
        issued: Long = 1_000,
        expires: Long = 1_020,
    ): String = JSONObject()
        .put("id", id).put("kind", kind).put("service", service).put("user", user)
        .put("requester", requester).put("action", "").put("command", command)
        .put("tty", tty).put("origin", origin).put("computer", computer).put("nonce", nonce)
        .put("issued_unix", issued).put("expires_unix", expires)
        .toString()

    @Test
    fun theSignedMessageIsTheRustSidesByteForByte() {
        // core/src/protocol/approve.rs pins the same text.
        val a = ApproveRule.parse(json())!!
        val expected = "21:pocketdesk-approve-v2\n3:a-1\n5:admin\n4:sudo\n5:alice\n5:alice\n0:\n" +
            "25:/usr/bin/sudo pacman -Syu\n10:/dev/pts/3\n34:sudo(812) < bash(700) < kitty(650)\n" +
            "64:$computer\n64:$nonce\n4:1000\n4:1020\n3:yes\n"
        assertArrayEquals(expected.toByteArray(Charsets.UTF_8), ApproveRule.message(a))
        // Lengths count UTF-8 bytes, as Rust's do: each escape bracket is three.
        val escaped = a.copy(command = "a⟨U+202E⟩b")
        assertTrue(String(ApproveRule.message(escaped), Charsets.UTF_8).contains("\n14:a⟨U+202E⟩b\n"))
        assertArrayEquals(MessageDigest.getInstance("SHA-256").digest(ApproveRule.message(a)), ApproveRule.pinChallenge(a))
    }

    @Test
    fun hiddenCharactersMustArriveEscaped() {
        assertTrue(ApproveRule.isShownText("a⟨U+202E⟩b", 100))
        assertTrue(ApproveRule.isShownText("", 100))
        assertFalse(ApproveRule.isShownText("a‮b", 100))
        assertFalse(ApproveRule.isShownText("⠀", 100))
        assertFalse(ApproveRule.isShownText("tab\there", 100))
        assertFalse(ApproveRule.isShownText("⟨U+12⟩", 100))
        assertFalse(ApproveRule.isShownText("x".repeat(101), 100))
        assertNull(ApproveRule.parse(json(command = "rm a‮b")))
        assertNull(ApproveRule.parse(json(origin = "kit‮ty(1)")))
        assertNotNull(ApproveRule.parse(json(command = "x".repeat(4000))))
    }

    @Test
    fun aMalformedRequestIsNotOffered() {
        assertNotNull(ApproveRule.parse(json()))
        assertNull(ApproveRule.parse(null))
        assertNull(ApproveRule.parse("not json"))
        assertNull(ApproveRule.parse(json(kind = "root")))
        assertNull("a pin names its own service", ApproveRule.parse(json(kind = "pin")))
        assertNotNull(ApproveRule.parse(json(kind = "pin", service = ApproveRule.PIN_SERVICE)))
        assertNull(ApproveRule.parse(json(service = "-x")))
        assertNull(ApproveRule.parse(json(user = "a b")))
        assertNull(ApproveRule.parse(json(requester = "a b")))
        assertNull(ApproveRule.parse(json(computer = "AB".repeat(32))))
        assertNull(ApproveRule.parse(json(nonce = "01")))
        assertNull(ApproveRule.parse(json(command = "two\nlines")))
        assertNull(ApproveRule.parse(json(expires = 999)))
        assertNull("a request waits a minute at most", ApproveRule.parse(json(expires = 1_061)))
    }

    @Test
    fun onlyWithTheSwitchOnAKeyAndInTimeIsItOffered() {
        assertEquals(ApproveRule.Decision.Offer, ApproveRule.decide(on = true, hasKey = true, pin = false, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.Off, ApproveRule.decide(on = false, hasKey = true, pin = false, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.NoKey, ApproveRule.decide(on = true, hasKey = false, pin = false, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.Offer, ApproveRule.decide(on = true, hasKey = false, pin = true, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.Expired, ApproveRule.decide(on = true, hasKey = true, pin = false, expiresUnix = 20, nowUnix = 20))
    }

    @Test
    fun aSignedYesVerifiesWithThePublicKeyAndNotForAnotherRequest() {
        val pair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
        val a = ApproveRule.parse(json())!!
        val signed = Signature.getInstance("SHA256withECDSA").apply {
            initSign(pair.private)
            update(ApproveRule.message(a))
        }.sign()
        val body = JSONObject(ApproveRule.signedAnswer(a.id, signed)!!)
        assertEquals("yes", body.getString("answer"))
        val sig = Base64.getDecoder().decode(body.getString("signature"))
        fun verifies(msg: ByteArray) = Signature.getInstance("SHA256withECDSA").run {
            initVerify(pair.public)
            update(msg)
            verify(sig)
        }
        assertTrue(verifies(ApproveRule.message(a)))
        assertFalse(verifies(ApproveRule.message(a.copy(nonce = "02".repeat(32)))))
        assertFalse(verifies(ApproveRule.message(a.copy(tty = "/dev/pts/4"))))
        assertNull(ApproveRule.signedAnswer("../x", signed))
        val key = JSONObject(ApproveRule.keyBody(pair.public.encoded, biometricOnly = true))
        assertEquals(91, Base64.getDecoder().decode(key.getString("public_key")).size)
        assertTrue(JSONObject(ApproveRule.keyBody(null, false)).isNull("public_key"))
        // All of the SHA-256, in sixteen groups of four.
        assertEquals(79, ApproveRule.fingerprint(pair.public.encoded).length)
        val pin = JSONObject(ApproveRule.pinAnswer("p-1", listOf(byteArrayOf(1), byteArrayOf(2)))!!)
        assertEquals(2, pin.getJSONArray("attestation").length())
        assertNull(ApproveRule.pinAnswer("p-1", emptyList()))
    }
}
