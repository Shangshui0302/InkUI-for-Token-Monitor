package dev.tokenmonitor.eink;

import android.content.Context;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyPermanentlyInvalidatedException;
import android.security.keystore.KeyProperties;
import android.util.AtomicFile;
import android.util.Base64;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.util.Enumeration;
import java.util.UUID;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/** A whole-record atomic encrypted update: secrets never enter SharedPreferences or Web storage. */
final class ConnectionStore {
    static final String KEY_ALIAS = "token-monitor.connection.v1";
    private final Storage storage;
    private Config current = empty();
    private String storageState = "unconfigured";
    private boolean rewritePending;

    /** Injectable record boundary; recovery never deletes an old record or its key. */
    interface Storage {
        StoredRecord read() throws Exception;
        void write(Config config) throws Exception;
        default void recover(Config config, boolean replaceKey) throws Exception {
            if (replaceKey) throw new IllegalStateException("key_recovery_unsupported");
            write(config);
        }
    }
    /** Only a missing key or Android's explicit permanent invalidation qualifies. */
    static final class KeyUnavailableException extends Exception {
        KeyUnavailableException() { super("connection_key_unavailable"); }
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
    private static Config empty() { return new Config("", "", 0); }
    ConnectionStore(Context context) { this(new EncryptedStorage(context)); }
    ConnectionStore(Storage storage) { this.storage = storage; load(); }

    synchronized void load() {
        Config previous = current;
        try {
            StoredRecord stored = storage.read();
            // Keep an in-process revision monotonic even when a previously loaded record vanishes.
            current = stored == null ? new Config("", "", previous.revision) : stored.config;
            storageState = stored == null ? "unconfigured" : "ready";
            rewritePending = stored != null && stored.needsRewrite;
        } catch (Exception error) {
            current = new Config("", "", previous.revision);
            storageState = permanentlyUnavailable(error) ? "key_unavailable" : "read_failed";
            rewritePending = false;
            return;
        }
        if (rewritePending) {
            try { storage.write(current); rewritePending = false; }
            catch (Exception ignored) {
                // The usable Hub and old atomic record survive a failed migration.
            }
        }
    }
    private static boolean permanentlyUnavailable(Throwable error) {
        for (Throwable cause = error; cause != null; cause = cause.getCause())
            if (cause instanceof KeyUnavailableException || cause instanceof KeyPermanentlyInvalidatedException) return true;
        return false;
    }
    synchronized Config snapshot() { return current; }
    synchronized String storageState() { return storageState; }
    synchronized boolean isCurrent(Config config) { return config.revision == current.revision; }
    synchronized JSONObject publicJson() throws Exception {
        return current.publicJson().put("storageState", storageState);
    }
    synchronized String reload() {
        load();
        try {
            JSONObject result = new JSONObject().put("ok", !unreadable()).put("connection", publicJson());
            if (unreadable()) result.put("error", storageState.equals("key_unavailable") ? "connection_key_unavailable" : "connection_read_failed");
            return result.toString();
        } catch (Exception ignored) { return failure("connection_read_failed"); }
    }
    private boolean unreadable() { return "read_failed".equals(storageState) || "key_unavailable".equals(storageState); }

    synchronized String save(String input) {
        try {
            JSONObject data = parse(input);
            boolean recovering = unreadable();
            if (recovering && !Boolean.TRUE.equals(data.opt("recover"))) return failure("connection_recovery_required");
            // An unreadable record cannot supply retained credentials. Recovery is always explicit.
            if (recovering && (!(data.opt("hubSecret") instanceof String) || requiredString(data, "hubSecret").isEmpty()))
                return failure("invalid_connection");
            Config candidate = candidate(data);
            if (recovering && candidate.hubUrl.isEmpty()) return failure("invalid_connection");
            boolean changed = recovering || !candidate.hubUrl.equals(current.hubUrl) || !candidate.hubSecret.equals(current.hubSecret);
            Config next = new Config(candidate.hubUrl, candidate.hubSecret,
                    changed ? Math.addExact(current.revision, 1) : current.revision);
            if (recovering) storage.recover(next, "key_unavailable".equals(storageState));
            else if (changed || rewritePending) storage.write(next);
            current = next; storageState = next.hubUrl.isEmpty() ? "unconfigured" : "ready"; rewritePending = false;
            return new JSONObject().put("ok", true).put("connection", publicJson()).toString();
        } catch (IllegalArgumentException | org.json.JSONException e) { return failure("invalid_connection"); }
        catch (Exception e) { return failure("connection_save_failed"); }
    }
    /** Validate an unsaved candidate; no persistence, revision change, or key generation. */
    synchronized Config candidate(String input) throws Exception { return candidate(parse(input)); }
    private Config candidate(JSONObject data) throws Exception {
        String hubUrl = NativePolicy.normalizeOrigin(requiredString(data, "hubUrl"));
        return new Config(hubUrl, nextSecret(data, "hubSecret", hubUrl, current.hubUrl, current.hubSecret), current.revision);
    }
    private static JSONObject parse(String input) throws Exception {
        if (input == null || input.length() > 24576) throw new IllegalArgumentException();
        return new JSONObject(input);
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

    static class EncryptedStorage implements Storage {
        private final AtomicFile file;
        private final String baseAlias;
        private String activeAlias;
        private boolean emptyRecord;
        EncryptedStorage(Context context) { this(context, "connection.v1", KEY_ALIAS); }
        // Package-scoped device checks use their own debug-app file and alias namespace.
        EncryptedStorage(Context context, String filename, String alias) {
            file = new AtomicFile(new File(context.getNoBackupFilesDir(), filename));
            baseAlias = alias; activeAlias = alias;
        }
        @Override public StoredRecord read() throws Exception {
            emptyRecord = false;
            byte[] stored;
            try (java.io.FileInputStream in = file.openRead()) { stored = NativePolicy.readLimited(in); }
            catch (java.io.FileNotFoundException missing) {
                if (!file.getBaseFile().exists()) { emptyRecord = true; return null; }
                throw missing;
            }
            JSONObject envelope = new JSONObject(new String(stored, StandardCharsets.UTF_8));
            String alias = envelope.optString("keyAlias", baseAlias);
            if (!allowedAlias(alias)) throw new IllegalArgumentException("invalid_key_alias");
            activeAlias = alias;
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, existingKey(alias), new GCMParameterSpec(128, Base64.decode(envelope.getString("iv"), Base64.NO_WRAP)));
            JSONObject data = new JSONObject(new String(cipher.doFinal(Base64.decode(envelope.getString("data"), Base64.NO_WRAP)), StandardCharsets.UTF_8));
            Config config = new Config(NativePolicy.normalizeOrigin(requiredString(data, "hubUrl")),
                    NativePolicy.checkSecret(requiredString(data, "hubSecret")), data.getLong("revision"));
            if (config.hubUrl.isEmpty() || config.revision < 0) throw new IllegalArgumentException("invalid_record");
            return new StoredRecord(config, data.length() != 3);
        }
        private boolean allowedAlias(String alias) { return alias.equals(baseAlias) || alias.startsWith(baseAlias + ".recovery."); }
        private static KeyStore keystore() throws Exception {
            KeyStore store = KeyStore.getInstance("AndroidKeyStore"); store.load(null); return store;
        }
        private static SecretKey existingKey(String alias) throws Exception {
            KeyStore store = keystore();
            if (!store.containsAlias(alias)) throw new KeyUnavailableException();
            java.security.Key key = store.getKey(alias, null);
            if (!(key instanceof SecretKey)) throw new KeyUnavailableException();
            return (SecretKey) key;
        }
        private static SecretKey generateKey(String alias) throws Exception {
            KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            generator.init(new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build());
            return generator.generateKey();
        }
        @Override public void write(Config config) throws Exception {
            // Only a genuinely empty install may create the initial key without recovery consent.
            SecretKey key;
            if (emptyRecord && !keystore().containsAlias(activeAlias)) key = generateKey(activeAlias);
            else key = existingKey(activeAlias);
            writeWithKey(config, activeAlias, key); emptyRecord = false;
        }
        @Override public void recover(Config config, boolean replaceKey) throws Exception {
            if (!replaceKey) {
                // If a damaged envelope lost its alias, reuse an available key from this store's
                // namespace. This path must never generate or delete any key.
                String alias = activeAlias;
                if (!keystore().containsAlias(alias)) {
                    Enumeration<String> aliases = keystore().aliases();
                    while (aliases.hasMoreElements()) {
                        String candidate = aliases.nextElement();
                        if (allowedAlias(candidate)) { alias = candidate; break; }
                    }
                }
                writeWithKey(config, alias, existingKey(alias)); activeAlias = alias; emptyRecord = false;
                return;
            }
            String replacement = baseAlias + ".recovery." + UUID.randomUUID();
            try {
                writeWithKey(config, replacement, generateKey(replacement));
                activeAlias = replacement; emptyRecord = false;
                // Keep old aliases: they may still be needed by an AtomicFile backup after a crash.
                // Only the newly committed envelope chooses the active key.
            } catch (Exception error) {
                try { keystore().deleteEntry(replacement); } catch (Exception ignored) { /* New orphan only. */ }
                throw error;
            }
        }
        private void writeWithKey(Config config, String alias, SecretKey key) throws Exception {
            JSONObject data = new JSONObject().put("hubUrl", config.hubUrl).put("hubSecret", config.hubSecret).put("revision", config.revision);
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding"); cipher.init(Cipher.ENCRYPT_MODE, key);
            byte[] encrypted = cipher.doFinal(data.toString().getBytes(StandardCharsets.UTF_8));
            byte[] envelope = new JSONObject().put("keyAlias", alias).put("iv", Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP))
                    .put("data", Base64.encodeToString(encrypted, Base64.NO_WRAP)).toString().getBytes(StandardCharsets.UTF_8);
            writeEnvelope(envelope);
        }
        void writeEnvelope(byte[] envelope) throws Exception {
            FileOutputStream out = null;
            try { out = file.startWrite(); out.write(envelope); file.finishWrite(out); }
            catch (Exception e) { if (out != null) file.failWrite(out); throw e; }
        }
    }
}
