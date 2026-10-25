package dev.tokenmonitor.eink;

/** Pure scheduling policy. Requests describe real visible DOM changes, never polling attempts. */
final class DisplayRefreshPolicy {
    enum Update { NONE, PARTIAL, FULL }
    static final int PAGES_PER_FULL_REFRESH = 6;
    static final int PARTIALS_PER_FULL_REFRESH = 30;
    static final long MIN_INTERVAL_MS = 180;
    static final long CONTENT_DELAY_MS = 120;

    private boolean active;
    private boolean destroyed;
    private int pagesSinceFull;
    private int partialsSinceFull;
    private Update pending = Update.NONE;
    private long dueAt;
    private long lastRefresh = -MIN_INTERVAL_MS;

    void resume(long now) {
        if (destroyed) return;
        active = true;
        request("full", now);
    }

    void pause() { active = false; pending = Update.NONE; }
    void destroy() { pause(); destroyed = true; }
    boolean isActive() { return active && !destroyed; }
    boolean hasPending() { return pending != Update.NONE; }

    boolean request(String reason, long now) {
        if (!isActive() || !("page".equals(reason) || "content".equals(reason) || "full".equals(reason))) return false;
        if ("page".equals(reason)) pagesSinceFull++;
        Update requested = "full".equals(reason) || pagesSinceFull >= PAGES_PER_FULL_REFRESH
                ? Update.FULL : Update.PARTIAL;
        long requestedAt = Math.max(now + ("content".equals(reason) ? CONTENT_DELAY_MS : 0),
                lastRefresh + MIN_INTERVAL_MS);
        // Content bursts share a deadline; page/full requests can move that deadline earlier.
        dueAt = pending == Update.NONE ? requestedAt : Math.min(dueAt, requestedAt);
        if (pending != Update.FULL) pending = requested;
        return true;
    }

    long delay(long now) { return Math.max(0, dueAt - now); }

    Update take(long now) {
        if (!isActive() || pending == Update.NONE || now < dueAt) return Update.NONE;
        // Count dispatched refreshes, so a content burst contributes only once. A full
        // refresh is due on the next real update after 30 partials, never on an idle timer.
        Update result = pending == Update.PARTIAL && partialsSinceFull >= PARTIALS_PER_FULL_REFRESH
                ? Update.FULL : pending;
        pending = Update.NONE;
        if (result == Update.FULL) {
            pagesSinceFull = 0;
            partialsSinceFull = 0;
        } else partialsSinceFull++;
        lastRefresh = now;
        return result;
    }
}
