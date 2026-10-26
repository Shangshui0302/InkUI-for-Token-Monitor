package dev.tokenmonitor.eink;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import java.util.function.LongSupplier;

/** One-use proof that the current Hub configuration passed a recovery test. */
final class RecoveryPermit {
    private static final long LIFETIME_MS = 30_000L;

    private final LongSupplier now;
    private byte[] fingerprint;
    private String storageState;
    private long revision;
    private long epoch;
    private long issuedAt;

    RecoveryPermit(LongSupplier now) {
        if (now == null) throw new NullPointerException("now");
        this.now = now;
    }

    synchronized void issue(String origin, String secret, String state, long revision, long epoch) {
        clear();
        String normalizedOrigin = NativePolicy.normalizeOrigin(origin);
        if (normalizedOrigin.isEmpty() || secret == null || state == null) return;
        this.fingerprint = fingerprint(normalizedOrigin, secret);
        this.storageState = state;
        this.revision = revision;
        this.epoch = epoch;
        this.issuedAt = now.getAsLong();
    }

    synchronized boolean consume(String origin, String secret, String state, long revision, long epoch) {
        if (fingerprint == null) return false;

        byte[] expectedFingerprint = fingerprint.clone();
        String expectedState = storageState;
        long expectedRevision = this.revision;
        long expectedEpoch = this.epoch;
        long expectedIssuedAt = issuedAt;
        clear();

        long elapsed = now.getAsLong() - expectedIssuedAt;
        if (elapsed < 0 || elapsed >= LIFETIME_MS) return false;
        if (state == null || !expectedState.equals(state) || expectedRevision != revision || expectedEpoch != epoch)
            return false;
        try {
            String normalizedOrigin = NativePolicy.normalizeOrigin(origin);
            if (normalizedOrigin.isEmpty() || secret == null) return false;
            return MessageDigest.isEqual(expectedFingerprint, fingerprint(normalizedOrigin, secret));
        } catch (IllegalArgumentException invalid) {
            return false;
        } finally {
            Arrays.fill(expectedFingerprint, (byte) 0);
        }
    }

    synchronized void clear() {
        if (fingerprint != null) Arrays.fill(fingerprint, (byte) 0);
        fingerprint = null;
        storageState = null;
        revision = 0;
        epoch = 0;
        issuedAt = 0;
    }

    private static byte[] fingerprint(String normalizedOrigin, String secret) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            update(digest, normalizedOrigin);
            update(digest, secret);
            return digest.digest();
        } catch (NoSuchAlgorithmException unavailable) {
            throw new IllegalStateException("SHA-256 unavailable", unavailable);
        }
    }

    private static void update(MessageDigest digest, String value) {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        digest.update(ByteBuffer.allocate(Integer.BYTES).putInt(bytes.length).array());
        digest.update(bytes);
        Arrays.fill(bytes, (byte) 0);
    }
}
