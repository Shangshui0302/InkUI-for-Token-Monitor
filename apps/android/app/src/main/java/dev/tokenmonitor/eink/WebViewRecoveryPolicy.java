package dev.tokenmonitor.eink;

import java.util.function.LongSupplier;

/** Limits automatic WebView recovery attempts to one per rolling minute. */
final class WebViewRecoveryPolicy {
    private static final long COOLDOWN_MS = 60_000L;

    private final LongSupplier now;
    private boolean hasAttempt;
    private long lastAttemptAt;

    WebViewRecoveryPolicy(LongSupplier now) {
        if (now == null) throw new NullPointerException("now");
        this.now = now;
    }

    synchronized boolean allowAutomatic() {
        long current = now.getAsLong();
        if (hasAttempt) {
            long elapsed = current - lastAttemptAt;
            if (elapsed < 0 || elapsed < COOLDOWN_MS) return false;
        }
        hasAttempt = true;
        lastAttemptAt = current;
        return true;
    }
}
