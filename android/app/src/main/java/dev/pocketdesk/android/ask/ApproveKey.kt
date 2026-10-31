package dev.pocketdesk.android.ask

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.Log
import androidx.annotation.RequiresApi
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * Approve with my phone: this phone's signing key, in the Android Keystore.
 * Made when the owner turns "Approve my computer's requests" on, never
 * leaves the Keystore, and signs only after the fingerprint (or, unless
 * "Fingerprint only" is on, the screen lock) for that one use. A new
 * fingerprint enrolled on the phone invalidates it, so the owner turns the
 * switch off and on and pins again on the computer.
 *
 * Android 11 (API 30) and later: before it, a key cannot take the screen
 * lock for one use.
 */
object ApproveKey {
    private const val TAG = "ApproveKey"
    private const val STORE = "AndroidKeyStore"
    private const val ALIAS = "pocketdesk-approve"

    /** Whether this Android can hold the key. */
    val supported: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R

    private fun store(): KeyStore = KeyStore.getInstance(STORE).apply { load(null) }

    fun exists(): Boolean = supported && runCatching { store().containsAlias(ALIAS) }.getOrDefault(false)

    /** The public key (X.509 SubjectPublicKeyInfo), or null when there is none. */
    fun publicKey(): ByteArray? =
        runCatching { store().getCertificate(ALIAS)?.publicKey?.encoded }.getOrNull()

    /**
     * Make a new key, replacing any: P-256, for signing only, each use
     * behind the fingerprint, or the fingerprint or screen lock. In
     * StrongBox where the phone has one. Its public key.
     */
    @RequiresApi(Build.VERSION_CODES.R)
    fun create(biometricOnly: Boolean): ByteArray {
        delete()
        val auth = if (biometricOnly) {
            KeyProperties.AUTH_BIOMETRIC_STRONG
        } else {
            KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL
        }
        fun spec(strongBox: Boolean) = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(true)
            // 0: every signature needs its own authentication.
            .setUserAuthenticationParameters(0, auth)
            .setInvalidatedByBiometricEnrollment(true)
            .setIsStrongBoxBacked(strongBox)
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

    fun delete() {
        runCatching { if (store().containsAlias(ALIAS)) store().deleteEntry(ALIAS) }
    }

    /** Why a signature cannot be started. */
    enum class Problem { NoKey, Invalidated }

    /**
     * A signature ready for the fingerprint prompt's CryptoObject, or why
     * not: [Problem.Invalidated] when a fingerprint was enrolled since the
     * key was made (the key is then deleted).
     */
    fun signature(): Result<Signature> {
        val key = runCatching { store().getKey(ALIAS, null) as? PrivateKey }.getOrNull()
            ?: return Result.failure(ProblemException(Problem.NoKey))
        return try {
            Result.success(Signature.getInstance("SHA256withECDSA").apply { initSign(key) })
        } catch (_: KeyPermanentlyInvalidatedException) {
            Log.i(TAG, "the approval key was invalidated by a new fingerprint")
            delete()
            Result.failure(ProblemException(Problem.Invalidated))
        } catch (e: java.security.GeneralSecurityException) {
            Result.failure(e)
        }
    }

    class ProblemException(val problem: Problem) : Exception(problem.name)
}
