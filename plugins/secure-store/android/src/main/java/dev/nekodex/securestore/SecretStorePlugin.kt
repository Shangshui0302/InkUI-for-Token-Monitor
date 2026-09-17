package dev.nekodex.securestore

import android.app.Activity
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.security.KeyStore
import java.util.concurrent.Executors
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONArray
import org.json.JSONObject

@InvokeArg
class SecretArgs {
    lateinit var key: String
    var value: IntArray? = null
}

@TauriPlugin
class SecretStorePlugin(private val activity: Activity) : Plugin(activity) {
    // One executor serializes reads, key generation and atomic replacement.
    private val io = Executors.newSingleThreadExecutor()
    private val recordKey = "nekodex/device/identity/v1"
    private val alias = "nekodex.device.identity.wrap.v1"
    private val aad = "nekodex/secret-store/aes-gcm/v1\u0000nekodex/device/identity/v1"
        .toByteArray(Charsets.UTF_8)

    private fun file() = AtomicFile(File(activity.noBackupFilesDir, "device-identity-v1.enc"))

    private fun wrappingKey(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (store.containsAlias(alias)) {
            return store.getKey(alias, null) as? SecretKey
                ?: throw IllegalStateException("Invalid wrapping key")
        }
        check(create) { "Wrapping key unavailable" }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder(alias,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(false)
                .setRandomizedEncryptionRequired(true)
                .build())
            generateKey()
        }
    }

    @Command
    fun readIdentity(invoke: Invoke) {
        io.execute {
            try {
                val args = invoke.parseArgs(SecretArgs::class.java)
                require(args.key == recordKey)
                val record = file()
                if (!record.baseFile.exists()) {
                    invoke.resolve(JSObject().apply { put("value", JSONObject.NULL) })
                    return@execute
                }
                // Envelope: format byte, 12-byte IV, 65-byte bundle, 16-byte tag.
                val envelope = record.openRead().use { stream ->
                    val buffer = ByteArray(95)
                    var count = 0
                    while (count < buffer.size) {
                        val read = stream.read(buffer, count, buffer.size - count)
                        if (read < 0) break
                        count += read
                    }
                    buffer.copyOf(count)
                }
                require(envelope.size == 94 && envelope[0] == 1.toByte())
                val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                cipher.init(Cipher.DECRYPT_MODE, wrappingKey(false),
                    GCMParameterSpec(128, envelope.copyOfRange(1, 13)))
                cipher.updateAAD(aad)
                val value = cipher.doFinal(envelope, 13, envelope.size - 13)
                try {
                    require(value.size == 65 && value[0] == 1.toByte())
                    invoke.resolve(JSObject().apply {
                        put("value", JSONArray(value.map { it.toInt() and 255 }))
                    })
                } finally { value.fill(0) }
            } catch (_: Exception) {
                invoke.reject("Android secret read failed")
            }
        }
    }

    @Command
    fun writeIdentity(invoke: Invoke) {
        io.execute {
            var value: ByteArray? = null
            try {
                val args = invoke.parseArgs(SecretArgs::class.java)
                require(args.key == recordKey)
                val input = requireNotNull(args.value)
                require(input.size == 65 && input[0] == 1 && input.all { it in 0..255 })
                val bytes = input.map { it.toByte() }.toByteArray()
                value = bytes
                input.fill(0)
                val record = file()
                val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                // Existing ciphertext with a missing alias must never be replaced.
                cipher.init(Cipher.ENCRYPT_MODE, wrappingKey(!record.baseFile.exists()))
                cipher.updateAAD(aad)
                val ciphertext = cipher.doFinal(bytes)
                require(cipher.iv.size == 12)
                val envelope = byteArrayOf(1) + cipher.iv + ciphertext
                val output = record.startWrite()
                try {
                    output.write(envelope)
                    output.fd.sync()
                    record.finishWrite(output)
                } catch (error: Exception) {
                    record.failWrite(output)
                    throw error
                }
                // AtomicFile can log a failed rename instead of throwing. Verify
                // the committed encrypted bytes before reporting durable success.
                val committed = record.openRead().use { stream ->
                    val buffer = ByteArray(envelope.size)
                    var count = 0
                    while (count < buffer.size) {
                        val read = stream.read(buffer, count, buffer.size - count)
                        if (read < 0) break
                        count += read
                    }
                    check(count == buffer.size && stream.read() == -1)
                    buffer
                }
                check(committed.contentEquals(envelope))
                invoke.resolve(JSObject().apply { put("value", JSONObject.NULL) })
            } catch (_: Exception) {
                invoke.reject("Android secret write failed")
            } finally { value?.fill(0) }
        }
    }
}
