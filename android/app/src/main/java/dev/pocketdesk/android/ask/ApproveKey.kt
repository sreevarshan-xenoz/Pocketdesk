package dev.pocketdesk.android.ask

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.core.content.edit
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * Approve with my phone: this phone's signing keys, in the Android Keystore.
 * The current key signs approvals; a pin makes a new key beside it, attested,
 * and the current one stays until a request from the computer names the new
 * key as pinned (review N13: a Yes to a pin request, perhaps one malware
 * made, never breaks the key that works until root has finished pinning).
 * Each key never leaves the Keystore and signs only after the fingerprint
 * (or, unless "Fingerprint only" is on, the screen lock) for that one use.
 * A new fingerprint enrolled on the phone invalidates them, so the owner
 * pins again.
 *
 * Android 11 (API 30) and later: before it, a key cannot take the screen
 * lock for one use.
 */
object ApproveKey {
    private const val TAG = "ApproveKey"
    private const val STORE = "AndroidKeyStore"
    private val ALIASES = listOf("pocketdesk-approve-a", "pocketdesk-approve-b")
    private const val PREFS = "pocketdesk-approve"
    private const val KEY_CURRENT = "current"

    /** Whether this Android can hold the key. */
    val supported: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R

    private fun store(): KeyStore = KeyStore.getInstance(STORE).apply { load(null) }

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** The alias of the key that signs now. */
    fun current(ctx: Context): String =
        prefs(ctx).getString(KEY_CURRENT, null)?.takeIf { it in ALIASES } ?: ALIASES[0]

    /** The other alias: where a pin's new key waits. */
    fun spare(ctx: Context): String = ALIASES.first { it != current(ctx) }

    private fun has(alias: String): Boolean = runCatching { store().containsAlias(alias) }.getOrDefault(false)

    fun exists(ctx: Context): Boolean = supported && (has(current(ctx)) || has(spare(ctx)))

    /** A key's public key (X.509 SubjectPublicKeyInfo), or null when there is none. */
    fun publicKey(alias: String): ByteArray? =
        runCatching { store().getCertificate(alias)?.publicKey?.encoded }.getOrNull()

    /** The key the computer's request names as pinned, made current if it was waiting; null for none. */
    fun pick(ctx: Context, pinned: List<String>): String? {
        val hex = { alias: String -> publicKey(alias)?.let { ApproveRule.fingerprint(it).replace(" ", "") } }
        val now = current(ctx)
        val next = spare(ctx)
        val nextHex = hex(next)
        if (nextHex != null && nextHex in pinned) {
            // Root finished the pin: the new key takes over.
            prefs(ctx).edit { putString(KEY_CURRENT, next) }
            delete(now)
            Log.i(TAG, "the pinned new approval key took over")
            return next
        }
        val nowHex = hex(now) ?: return null
        return if (pinned.isEmpty() || nowHex in pinned) now else null
    }

    /**
     * Make a key at [alias], replacing what is there: P-256, for signing
     * only, each use behind the fingerprint, or the fingerprint or screen
     * lock; with a pin's [challenge], attested. In StrongBox where the phone
     * has one. Its public key.
     */
    @RequiresApi(Build.VERSION_CODES.R)
    fun create(alias: String, biometricOnly: Boolean, challenge: ByteArray? = null): ByteArray {
        delete(alias)
        val auth = if (biometricOnly) {
            KeyProperties.AUTH_BIOMETRIC_STRONG
        } else {
            KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL
        }
        fun spec(strongBox: Boolean) = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(true)
            // 0: every signature needs its own authentication.
            .setUserAuthenticationParameters(0, auth)
            .setInvalidatedByBiometricEnrollment(true)
            .setIsStrongBoxBacked(strongBox)
            // A pin's challenge: the key's attestation chain then proves to
            // root, up to Google's roots, that it is this key, in hardware.
            .apply { if (challenge != null) setAttestationChallenge(challenge) }
            .build()
        val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, STORE)
        val pair = try {
            generator.initialize(spec(strongBox = true))
            generator.generateKeyPair()
        } catch (_: StrongBoxUnavailableException) {
            generator.initialize(spec(strongBox = false))
            generator.generateKeyPair()
        }
        Log.i(TAG, "an approval key was made")
        return pair.public.encoded
    }

    /** A key's certificate chain, leaf first: its attestation, when made with a challenge. */
    fun chain(alias: String): List<ByteArray> =
        runCatching { store().getCertificateChain(alias)?.map { it.encoded } }.getOrNull().orEmpty()

    fun delete(alias: String) {
        runCatching { if (store().containsAlias(alias)) store().deleteEntry(alias) }
    }

    /** Both keys: the feature was turned off. */
    fun deleteAll(ctx: Context) {
        ALIASES.forEach(::delete)
        prefs(ctx).edit { remove(KEY_CURRENT) }
    }

    /** Why a signature cannot be started. */
    enum class Problem { NoKey, Invalidated, NotPinned }

    /**
     * A signature by [alias] ready for the fingerprint prompt's
     * CryptoObject, or why not: [Problem.Invalidated] when a fingerprint
     * was enrolled since the key was made (the key is then deleted).
     */
    fun signature(alias: String): Result<Signature> {
        val key = runCatching { store().getKey(alias, null) as? PrivateKey }.getOrNull()
            ?: return Result.failure(ProblemException(Problem.NoKey))
        return try {
            Result.success(Signature.getInstance("SHA256withECDSA").apply { initSign(key) })
        } catch (_: KeyPermanentlyInvalidatedException) {
            Log.i(TAG, "the approval key was invalidated by a new fingerprint")
            delete(alias)
            Result.failure(ProblemException(Problem.Invalidated))
        } catch (e: java.security.GeneralSecurityException) {
            Result.failure(e)
        }
    }

    class ProblemException(val problem: Problem) : Exception(problem.name)
}
