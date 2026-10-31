package dev.pocketdesk.android.ask

import java.security.KeyPairGenerator
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
        command: String = "sudo pacman -Syu",
        computer: String = this.computer,
        nonce: String = this.nonce,
        issued: Long = 1_000,
        expires: Long = 1_020,
    ): String = JSONObject()
        .put("id", id).put("kind", kind).put("service", service).put("user", user)
        .put("command", command).put("computer", computer).put("nonce", nonce)
        .put("issued_unix", issued).put("expires_unix", expires)
        .toString()

    @Test
    fun theSignedMessageIsTheRustSidesByteForByte() {
        // core/src/protocol/approve.rs pins the same text.
        val a = ApproveRule.parse(json())!!
        val expected = "21:pocketdesk-approve-v1\n3:a-1\n5:admin\n4:sudo\n5:alice\n16:sudo pacman -Syu\n" +
            "64:$computer\n64:$nonce\n4:1000\n4:1020\n3:yes\n"
        assertArrayEquals(expected.toByteArray(Charsets.UTF_8), ApproveRule.message(a))
        // Lengths count UTF-8 bytes, as Rust's do.
        val wide = a.copy(command = "é")
        assertTrue(String(ApproveRule.message(wide), Charsets.UTF_8).contains("\n2:é\n"))
    }

    @Test
    fun aMalformedRequestIsNotOffered() {
        assertNotNull(ApproveRule.parse(json()))
        assertNull(ApproveRule.parse(null))
        assertNull(ApproveRule.parse("not json"))
        assertNull(ApproveRule.parse(json(kind = "root")))
        assertNull(ApproveRule.parse(json(service = "-x")))
        assertNull(ApproveRule.parse(json(user = "a b")))
        assertNull(ApproveRule.parse(json(computer = "AB".repeat(32))))
        assertNull(ApproveRule.parse(json(nonce = "01")))
        assertNull(ApproveRule.parse(json(command = "two\nlines")))
        assertNull(ApproveRule.parse(json(expires = 999)))
        assertNull("a request waits a minute at most", ApproveRule.parse(json(expires = 1_061)))
    }

    @Test
    fun onlyWithTheSwitchOnAKeyAndInTimeIsItOffered() {
        assertEquals(ApproveRule.Decision.Offer, ApproveRule.decide(on = true, hasKey = true, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.Off, ApproveRule.decide(on = false, hasKey = true, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.NoKey, ApproveRule.decide(on = true, hasKey = false, expiresUnix = 20, nowUnix = 10))
        assertEquals(ApproveRule.Decision.Expired, ApproveRule.decide(on = true, hasKey = true, expiresUnix = 20, nowUnix = 20))
    }

    @Test
    fun aSignedYesVerifiesWithThePublicKeyAndNotForAnotherRequest() {
        // The Keystore's key and signature, as a software P-256 key makes them.
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
        assertFalse(verifies(ApproveRule.message(a.copy(command = "sudo rm -rf /"))))
        assertNull(ApproveRule.signedAnswer("../x", signed))
        // The key goes as X.509 SubjectPublicKeyInfo: 91 bytes for P-256.
        val key = JSONObject(ApproveRule.keyBody(pair.public.encoded, biometricOnly = true))
        assertEquals(91, Base64.getDecoder().decode(key.getString("public_key")).size)
        assertTrue(key.getBoolean("biometric_only"))
        assertTrue(JSONObject(ApproveRule.keyBody(null, false)).isNull("public_key"))
        assertEquals(19, ApproveRule.fingerprint(pair.public.encoded).length)
    }
}
