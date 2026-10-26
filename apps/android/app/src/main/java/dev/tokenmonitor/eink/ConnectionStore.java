package dev.tokenmonitor.eink;

import android.content.Context;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.AtomicFile;
import android.util.Base64;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/** A whole-record atomic encrypted update: secrets never enter SharedPreferences or Web storage. */
final class ConnectionStore {
    private static final String KEY_ALIAS = "token-monitor.connection.v1";
    private final Storage storage;
    private Config current = new Config("", "", 0);
    private boolean readable = true;
    private boolean rewritePending;

    /** Injectable record boundary for migration checks without Android storage or real credentials. */
    interface Storage {
        StoredRecord read() throws Exception;
        void write(Config config) throws Exception;
    }
    static final class StoredRecord {
        final Config config;
        final boolean needsRewrite;
        StoredRecord(Config config, boolean needsRewrite) {
            this.config = config; this.needsRewrite = needsRewrite;
        }
    }

    static final class Config {
        final String hubUrl, hubSecret;
        final long revision;
        Config(String hubUrl, String hubSecret, long revision) {
            this.hubUrl = hubUrl; this.hubSecret = hubSecret; this.revision = revision;
        }
        JSONObject publicJson() throws Exception {
            return new JSONObject().put("hubUrl", hubUrl).put("hasHubSecret", !hubSecret.isEmpty())
                    .put("revision", Long.toString(revision));
        }
    }

    ConnectionStore(Context context) { this(new EncryptedStorage(context)); }

    ConnectionStore(Storage storage) {
        this.storage = storage;
        try {
            StoredRecord stored = storage.read();
            if (stored == null) return;
            current = stored.config;
            rewritePending = stored.needsRewrite;
        } catch (Exception ignored) {
            // A lost/invalidated Keystore key must never send partially recovered credentials.
            readable = false;
            return;
        }
        if (rewritePending) {
            try { storage.write(current); rewritePending = false; }
            catch (Exception ignored) {
                // The validated Hub remains usable. Atomic storage keeps the old record for retry
                // on the next save or launch; migration failure must not clear Hub credentials.
            }
        }
    }

    synchronized Config snapshot() { return current; }
    synchronized boolean isCurrent(Config config) { return config.revision == current.revision; }

    synchronized String save(String input) {
        try {
            Config candidate = candidate(input);
            String hubUrl = candidate.hubUrl, hubSecret = candidate.hubSecret;
            boolean changed = !readable || !hubUrl.equals(current.hubUrl) || !hubSecret.equals(current.hubSecret);
            Config next = new Config(hubUrl, hubSecret,
                    changed ? Math.addExact(current.revision, 1) : current.revision);
            if (changed || rewritePending) storage.write(next);
            current = next; readable = true; rewritePending = false;
            return new JSONObject().put("ok", true).put("connection", next.publicJson()).toString();
        } catch (IllegalArgumentException | org.json.JSONException e) { return failure("invalid_connection"); }
        catch (Exception e) { return failure("connection_save_failed"); }
    }

    /** Validate an unsaved candidate against one stored snapshot; never persist or advance revision. */
    synchronized Config candidate(String input) throws Exception {
        if (input == null || input.length() > 24576) throw new IllegalArgumentException();
        JSONObject data = new JSONObject(input);
        String hubUrl = NativePolicy.normalizeOrigin(requiredString(data, "hubUrl"));
        return new Config(hubUrl, nextSecret(data, "hubSecret", hubUrl, current.hubUrl, current.hubSecret),
                current.revision);
    }

    private static String requiredString(JSONObject data, String name) throws Exception {
        Object value = data.opt(name);
        if (!(value instanceof String)) throw new IllegalArgumentException();
        return (String) value;
    }
    private static String nextSecret(JSONObject data, String name, String origin, String previousOrigin, String previousSecret) throws Exception {
        String candidate = !data.has(name) || data.isNull(name) ? null : requiredString(data, name);
        return NativePolicy.selectSecret(candidate, origin, previousOrigin, previousSecret);
    }
    private static String failure(String code) { return "{\"ok\":false,\"error\":\"" + code + "\"}"; }
    private static final class EncryptedStorage implements Storage {
        private final AtomicFile file;
        EncryptedStorage(Context context) {
            file = new AtomicFile(new File(context.getNoBackupFilesDir(), "connection.v1"));
        }
        @Override public StoredRecord read() throws Exception {
            byte[] stored;
            try (java.io.FileInputStream in = file.openRead()) { stored = NativePolicy.readLimited(in); }
            catch (java.io.FileNotFoundException missing) {
                if (!file.getBaseFile().exists()) return null;
                throw missing;
            }
            JSONObject envelope = new JSONObject(new String(stored, StandardCharsets.UTF_8));
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, Base64.decode(envelope.getString("iv"), Base64.NO_WRAP)));
            JSONObject data = new JSONObject(new String(cipher.doFinal(Base64.decode(envelope.getString("data"), Base64.NO_WRAP)), StandardCharsets.UTF_8));
            Config config = new Config(NativePolicy.normalizeOrigin(requiredString(data, "hubUrl")),
                    NativePolicy.checkSecret(requiredString(data, "hubSecret")), data.getLong("revision"));
            // Older records may contain retired or malformed settings. Keep only the Hub record.
            return new StoredRecord(config, data.length() != 3);
        }
        private static SecretKey key() throws Exception {
            KeyStore store = KeyStore.getInstance("AndroidKeyStore"); store.load(null);
            if (store.containsAlias(KEY_ALIAS)) return (SecretKey) store.getKey(KEY_ALIAS, null);
            KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            generator.init(new KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build());
            return generator.generateKey();
        }
        @Override public void write(Config config) throws Exception {
            JSONObject data = new JSONObject().put("hubUrl", config.hubUrl).put("hubSecret", config.hubSecret)
                    .put("revision", config.revision);
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding"); cipher.init(Cipher.ENCRYPT_MODE, key());
            byte[] encrypted = cipher.doFinal(data.toString().getBytes(StandardCharsets.UTF_8));
            byte[] envelope = new JSONObject().put("iv", Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP))
                    .put("data", Base64.encodeToString(encrypted, Base64.NO_WRAP)).toString().getBytes(StandardCharsets.UTF_8);
            FileOutputStream out = null;
            try { out = file.startWrite(); out.write(envelope); file.finishWrite(out); }
            catch (Exception e) { if (out != null) file.failWrite(out); throw e; }
        }
    }
}
