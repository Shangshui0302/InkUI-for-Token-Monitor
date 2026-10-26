package dev.tokenmonitor.eink;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

/** Synthetic transport only; no sockets, listeners, Android runtime or real credentials. */
public final class HubHttpClientTest {
    public static void main(String[] args) throws Exception {
        policyAndValidation();
        slowDripHasTotalDeadline();
        cancellationDoesNotWaitForDisconnect();
        sharedSlotsSurviveClientRecreation();
        staleAndBlockedValidation();
        System.out.println("Hub HTTP client checks passed (policy, deadline, cancellation, physical bound, stale results)");
    }

    private static void policyAndValidation() throws Exception {
        try (Harness test = new Harness(1000, 2)) {
            for (String path : new String[]{"/api/stats", "/api/history"}) {
                FakeConnection connection = test.add();
                HubHttpClient.Result result = test.client.execute(path, "HTTPS://Candidate:443/", "synthetic-secret", path, () -> true);
                check(result.error == null, "allowed route succeeds");
                equal("valid", new String(result.body, StandardCharsets.UTF_8));
                equal("https://candidate" + path, test.opened.take());
                equal(path, test.validated.take());
                equal("GET", connection.getRequestMethod());
                equal("application/json", connection.getRequestProperty("Accept"));
                equal("identity", connection.getRequestProperty("Accept-Encoding"));
                equal("Bearer synthetic-secret", connection.getRequestProperty("Authorization"));
                equal("", connection.getRequestProperty("Cookie"));
                equal("", connection.getRequestProperty("Cookie2"));
                check(!connection.getInstanceFollowRedirects() && !connection.getUseCaches(), "redirect/cache policy");
                check(connection.getReadTimeout() <= 8000 && connection.getConnectTimeout() <= 8000, "per-operation timeout cap");
            }
            int opens = test.opens.get();
            for (String path : new String[]{"/api/stats?x=1", "/api/history/", "/api/write", "https://other/api/stats"})
                equal("invalid_connection", test.client.execute("invalid-path", "https://candidate", "", path, () -> true).error);
            for (String origin : new String[]{"https://user@candidate", "https://candidate/api", "file:///etc/passwd"})
                equal("invalid_connection", test.client.execute("invalid-origin", origin, "", "/api/stats", () -> true).error);
            equal("invalid_connection", test.client.execute("invalid-secret", "https://candidate", "bad\r\nsecret", "/api/stats", () -> true).error);
            equal("unconfigured", test.client.execute("empty", "", "", "/api/stats", () -> true).error);
            equal("cancelled", test.client.execute("stale", "https://candidate", "", "/api/stats", () -> false).error);
            check(opens == test.opens.get(), "rejected inputs never open connection");
        }
    }

    private static void slowDripHasTotalDeadline() throws Exception {
        try (Harness test = new Harness(120, 1)) {
            FakeConnection connection = test.add(); connection.slowDrip = true; connection.ignoreDisconnect = true;
            long started = System.nanoTime();
            HubHttpClient.Result result = test.client.execute("drip", "http://candidate", "", "/api/stats", () -> true);
            equal("timeout", result.error);
            check(elapsed(started) < 600, "total deadline stops repeated short reads");
            check(connection.reads.get() > 1, "drip actually delivered repeated bytes");
            check(result.body == null, "deadline exposes no partial body");
            check(connection.streamClosed.await(300, TimeUnit.MILLISECONDS), "cancelled drip exits between reads even if disconnect has no effect");
        }
    }

    private static void cancellationDoesNotWaitForDisconnect() throws Exception {
        try (Harness test = new Harness(1500, 2)) {
            FakeConnection one = test.add(), two = test.add();
            one.blockResponse = two.blockResponse = true;
            one.blockDisconnect = two.blockDisconnect = true;
            Future<HubHttpClient.Result> first = test.request("old-one", () -> true);
            Future<HubHttpClient.Result> second = test.request("old-two", () -> true);
            check(one.entered.await(1, TimeUnit.SECONDS) && two.entered.await(1, TimeUnit.SECONDS), "both physical slots occupied");
            long started = System.nanoTime();
            test.client.cancel("old-one"); test.client.cancelAll();
            check(elapsed(started) < 100, "cancel does not run disconnect on caller");
            equal("cancelled", first.get(300, TimeUnit.MILLISECONDS).error);
            equal("cancelled", second.get(300, TimeUnit.MILLISECONDS).error);
            check(one.cleaning.await(1, TimeUnit.SECONDS) && two.cleaning.await(1, TimeUnit.SECONDS), "cleanup attempted asynchronously");
            for (int i = 0; i < 100; i++) {
                long busyStarted = System.nanoTime();
                equal("network_error", test.client.execute("busy-" + i, "http://candidate", "", "/api/stats", () -> true).error);
                check(elapsed(busyStarted) < 100, "busy attempts are not queued");
            }
            check(test.opens.get() == 2, "stuck old operations cannot grow physical concurrency");
            started = System.nanoTime(); test.client.close();
            check(elapsed(started) < 100, "close does not wait for disconnect");
            equal("cancelled", test.client.execute("closed", "http://candidate", "", "/api/stats", () -> true).error);
            one.release.countDown(); two.release.countDown();
            one.releaseCleanup.countDown(); two.releaseCleanup.countDown();
            check(first.get().body == null && second.get().body == null, "late old successes remain cancelled");
        }
    }

    private static void sharedSlotsSurviveClientRecreation() throws Exception {
        Semaphore processSlots = new Semaphore(2);
        try (Harness oldActivity = new Harness(1500, 2, processSlots);
                Harness recreatedActivity = new Harness(1500, 2, processSlots)) {
            FakeConnection one = oldActivity.add(), two = oldActivity.add();
            one.blockResponse = two.blockResponse = true;
            one.blockDisconnect = two.blockDisconnect = true;
            Future<HubHttpClient.Result> first = oldActivity.request("activity-one", () -> true);
            Future<HubHttpClient.Result> second = oldActivity.request("activity-two", () -> true);
            check(one.entered.await(1, TimeUnit.SECONDS) && two.entered.await(1, TimeUnit.SECONDS), "old Activity holds shared slots");
            oldActivity.client.close();
            equal("cancelled", first.get(300, TimeUnit.MILLISECONDS).error);
            equal("cancelled", second.get(300, TimeUnit.MILLISECONDS).error);
            check(one.cleaning.await(1, TimeUnit.SECONDS) && two.cleaning.await(1, TimeUnit.SECONDS), "old cleanup remains blocked after close");
            recreatedActivity.add();
            for (int i = 0; i < 20; i++)
                equal("network_error", recreatedActivity.client.execute("recreated-" + i, "http://candidate", "", "/api/stats", () -> true).error);
            check(oldActivity.opens.get() == 2 && recreatedActivity.opens.get() == 0,
                    "recreating a client cannot replenish occupied process slots");
            one.release.countDown(); two.release.countDown();
            one.releaseCleanup.countDown(); two.releaseCleanup.countDown();
            long until = System.nanoTime() + TimeUnit.SECONDS.toNanos(1);
            while (processSlots.availablePermits() != 2 && System.nanoTime() < until) Thread.sleep(1);
            check(processSlots.availablePermits() == 2, "shared slots release only when physical cleanup finishes");
            check(recreatedActivity.client.execute("recovered", "http://candidate", "", "/api/stats", () -> true).error == null,
                    "recreated Activity can read once old physical work finishes");
        }
    }

    private static void staleAndBlockedValidation() throws Exception {
        try (Harness test = new Harness(1000, 1)) {
            FakeConnection connection = test.add(); connection.blockResponse = true;
            AtomicBoolean current = new AtomicBoolean(true);
            Future<HubHttpClient.Result> pending = test.request("revision", current::get);
            check(connection.entered.await(1, TimeUnit.SECONDS), "revision request entered");
            current.set(false);
            equal("cancelled", pending.get(300, TimeUnit.MILLISECONDS).error);
            connection.release.countDown();
            check(pending.get().body == null, "stale result has no body");
        }
        try (Harness test = new Harness(120, 1)) {
            test.add(); test.blockValidation = true;
            Future<HubHttpClient.Result> pending = test.request("validator", () -> true);
            check(test.validationEntered.await(1, TimeUnit.SECONDS), "validator entered");
            equal("timeout", pending.get(500, TimeUnit.MILLISECONDS).error);
            equal("network_error", test.client.execute("validator-busy", "http://candidate", "", "/api/stats", () -> true).error);
            test.releaseValidation.countDown();
        }
    }

    private static final class Harness implements AutoCloseable {
        final BlockingQueue<FakeConnection> pending = new LinkedBlockingQueue<>();
        final BlockingQueue<String> opened = new LinkedBlockingQueue<>(), validated = new LinkedBlockingQueue<>();
        final AtomicInteger opens = new AtomicInteger();
        final List<FakeConnection> connections = new ArrayList<>();
        final ExecutorService callers = Executors.newFixedThreadPool(2);
        final CountDownLatch validationEntered = new CountDownLatch(1), releaseValidation = new CountDownLatch(1);
        volatile boolean blockValidation;
        final HubHttpClient client;
        Harness(long deadline, int concurrency) {
            this(deadline, concurrency, new Semaphore(concurrency));
        }
        Harness(long deadline, int concurrency, Semaphore slots) {
            client = new HubHttpClient(url -> {
                opens.incrementAndGet(); opened.add(url);
                FakeConnection next = pending.poll();
                if (next == null) throw new AssertionError("unexpected connection");
                return next;
            }, (body, path) -> {
                validated.add(path); validationEntered.countDown();
                if (blockValidation) uninterruptible(releaseValidation);
                if (!"valid".equals(new String(body, StandardCharsets.UTF_8))) throw new IOException();
            }, ConnectionTestController::classifyFailure, deadline, concurrency, slots);
        }
        FakeConnection add() throws Exception {
            FakeConnection next = new FakeConnection(); connections.add(next); pending.add(next); return next;
        }
        Future<HubHttpClient.Result> request(String id, java.util.function.BooleanSupplier current) {
            return callers.submit(() -> client.execute(id, "http://candidate", "", "/api/stats", current));
        }
        @Override public void close() {
            releaseValidation.countDown();
            for (FakeConnection connection : connections) {
                connection.release.countDown(); connection.releaseCleanup.countDown();
            }
            client.close(); callers.shutdownNow();
        }
    }
    private static final class FakeConnection extends HttpURLConnection {
        final CountDownLatch entered = new CountDownLatch(1), release = new CountDownLatch(1);
        final CountDownLatch cleaning = new CountDownLatch(1), releaseCleanup = new CountDownLatch(1);
        final AtomicInteger reads = new AtomicInteger();
        final CountDownLatch streamClosed = new CountDownLatch(1);
        volatile boolean disconnected;
        boolean blockResponse, blockDisconnect, slowDrip, ignoreDisconnect;
        FakeConnection() throws Exception { super(new URL("http://synthetic")); }
        @Override public int getResponseCode() {
            entered.countDown();
            if (blockResponse) uninterruptible(release);
            return 200;
        }
        @Override public long getContentLengthLong() { return -1; }
        @Override public InputStream getInputStream() {
            if (!slowDrip) return new ByteArrayInputStream("valid".getBytes(StandardCharsets.UTF_8));
            return new InputStream() {
                @Override public void close() { streamClosed.countDown(); }
                @Override public int read() {
                    long until = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(10);
                    while (System.nanoTime() < until) {
                        try { Thread.sleep(1); } catch (InterruptedException ignored) { }
                    }
                    reads.incrementAndGet(); return disconnected ? -1 : 'x';
                }
                @Override public int read(byte[] bytes, int offset, int length) {
                    int next = read();
                    if (next == -1) return -1;
                    bytes[offset] = (byte) next; return 1;
                }
            };
        }
        @Override public void disconnect() {
            cleaning.countDown();
            if (blockDisconnect) uninterruptible(releaseCleanup);
            if (!ignoreDisconnect) disconnected = true;
        }
        @Override public boolean usingProxy() { return false; }
        @Override public void connect() { }
    }
    private static void uninterruptible(CountDownLatch latch) {
        for (;;) {
            try { latch.await(); return; } catch (InterruptedException ignored) { }
        }
    }
    private static long elapsed(long started) { return TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started); }
    private static void check(boolean condition, String message) { if (!condition) throw new AssertionError(message); }
    private static void equal(String expected, String actual) {
        if (!expected.equals(actual)) throw new AssertionError(expected + " != " + actual);
    }
}
