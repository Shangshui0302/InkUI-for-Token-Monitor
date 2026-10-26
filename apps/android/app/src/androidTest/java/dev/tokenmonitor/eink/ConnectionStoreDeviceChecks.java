package dev.tokenmonitor.eink;

import android.content.Context;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyStore;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Enumeration;
import java.util.List;
import java.util.UUID;

/** Real Keystore checks. Only a unique synthetic record in the debug app is touched. */
public final class ConnectionStoreDeviceChecks {
    public static void run(Context context) throws Exception {
        check(context.getPackageName().endsWith(".debug"), "debug application required");
        String suffix = UUID.randomUUID().toString();
        String filename = "connection.device-check." + suffix;
        String alias = "token-monitor.device-check." + suffix;
        File record = new File(context.getNoBackupFilesDir(), filename);
        try {
            ConnectionStore empty = store(context, filename, alias);
            check(empty.storageState().equals("unconfigured"), "empty install state");
            check(aliases(alias).isEmpty(), "first read creates no key");
            String first = payload("http://synthetic-device-check.invalid:17321", "synthetic-device-check-secret", false);
            check(ok(empty.save(first)), "first encrypted save");
            check(empty.storageState().equals("ready"), "saved record ready");
            byte[] encrypted = Files.readAllBytes(record.toPath());
            String disk = new String(encrypted, StandardCharsets.UTF_8);
            check(!disk.contains("synthetic-device-check-secret") && !disk.contains("synthetic-device-check.invalid"), "disk contains no plaintext credential or origin");
            ConnectionStore cold = store(context, filename, alias);
            check(cold.snapshot().hubSecret.equals("synthetic-device-check-secret"), "cold process model decrypts secret");
            check(cold.snapshot().hubUrl.equals("http://synthetic-device-check.invalid:17321"), "cold process model decrypts origin");
            check(!cold.publicJson().toString().contains("synthetic-device-check-secret"), "public projection excludes secret");

            // A malformed record is distinct from missing key and survives retry and rejected save.
            byte[] corrupt = "{synthetic-corrupt-record".getBytes(StandardCharsets.UTF_8);
            write(record, corrupt);
            ConnectionStore damaged = store(context, filename, alias);
            check(damaged.storageState().equals("read_failed"), "malformed record read state");
            check(!ok(damaged.reload()), "bad record reload fails explicitly");
            check(!ok(damaged.save(first)), "unconfirmed replacement refused");
            check(Arrays.equals(corrupt, Files.readAllBytes(record.toPath())), "bad record not automatically removed or overwritten");
            List<String> priorAliases = aliases(alias);
            String replacement = payload("http://synthetic-replacement.invalid:17321", "synthetic-replacement-secret", true);
            check(ok(damaged.save(replacement)), "confirmed read-failure recovery");
            check(priorAliases.equals(aliases(alias)), "read-failure recovery reuses existing key");
            check(store(context, filename, alias).snapshot().hubSecret.equals("synthetic-replacement-secret"), "recovered record cold read");

            // Delete only this fixture's key to emulate an explicit permanent loss.
            byte[] oldRecord = Files.readAllBytes(record.toPath());
            String oldAlias = new JSONObject(new String(oldRecord, StandardCharsets.UTF_8)).getString("keyAlias");
            keys().deleteEntry(oldAlias);
            ConnectionStore unavailable = store(context, filename, alias);
            check(unavailable.storageState().equals("key_unavailable"), "missing key state");
            check(aliases(alias).isEmpty(), "missing key read does not recreate alias");
            check(!ok(unavailable.reload()), "missing key retry remains explicit");
            check(aliases(alias).isEmpty(), "retry does not recreate key");
            check(!ok(unavailable.save(first)), "missing key needs explicit replacement confirmation");
            check(!ok(unavailable.save(payload("http://synthetic-replacement.invalid:17321", "", true))), "recovery requires new secret");
            check(Arrays.equals(oldRecord, Files.readAllBytes(record.toPath())), "missing key retry preserves encrypted record");

            // Exercise a real newly generated key plus encryption, then fail before commit.
            ConnectionStore failed = new ConnectionStore(new ConnectionStore.EncryptedStorage(context, filename, alias) {
                @Override void writeEnvelope(byte[] envelope) throws Exception { throw new java.io.IOException("synthetic-write-failure"); }
            });
            check(!ok(failed.save(replacement)), "failed replacement reports failure");
            check(failed.storageState().equals("key_unavailable"), "failed replacement retains recovery state");
            check(aliases(alias).isEmpty(), "failed replacement removes only its newly generated orphan key");
            check(Arrays.equals(oldRecord, Files.readAllBytes(record.toPath())), "failed replacement keeps old ciphertext");

            check(ok(unavailable.save(replacement)), "confirmed missing-key recovery succeeds");
            check(aliases(alias).size() == 1, "confirmed replacement commits one new key");
            ConnectionStore recovered = store(context, filename, alias);
            check(recovered.storageState().equals("ready"), "recovered store ready after cold read");
            check(recovered.snapshot().hubSecret.equals("synthetic-replacement-secret"), "new key cold decryption");
            check(!new String(Files.readAllBytes(record.toPath()), StandardCharsets.UTF_8).contains("synthetic-replacement-secret"), "replacement ciphertext excludes plaintext");
        } finally {
            // All names are generated above; never touch the app's actual connection.v1 record.
            new android.util.AtomicFile(record).delete();
            for (String key : aliases(alias)) keys().deleteEntry(key);
        }
    }
    private static ConnectionStore store(Context context, String filename, String alias) {
        return new ConnectionStore(new ConnectionStore.EncryptedStorage(context, filename, alias));
    }
    private static String payload(String url, String secret, boolean recover) throws Exception {
        return new JSONObject().put("hubUrl", url).put("hubSecret", secret).put("recover", recover).toString();
    }
    private static boolean ok(String result) throws Exception { return new JSONObject(result).optBoolean("ok"); }
    private static KeyStore keys() throws Exception { KeyStore store = KeyStore.getInstance("AndroidKeyStore"); store.load(null); return store; }
    private static List<String> aliases(String prefix) throws Exception {
        List<String> result = new ArrayList<>();
        Enumeration<String> names = keys().aliases();
        while (names.hasMoreElements()) {
            String name = names.nextElement();
            if (name.equals(prefix) || name.startsWith(prefix + ".recovery.")) result.add(name);
        }
        java.util.Collections.sort(result); return result;
    }
    private static void write(File file, byte[] bytes) throws Exception {
        try (FileOutputStream out = new FileOutputStream(file)) { out.write(bytes); }
    }
    private static void check(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
}
