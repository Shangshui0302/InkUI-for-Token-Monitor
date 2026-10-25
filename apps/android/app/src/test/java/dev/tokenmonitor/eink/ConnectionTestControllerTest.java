package dev.tokenmonitor.eink;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.ConnectException;
import java.net.HttpURLConnection;
import java.net.SocketTimeoutException;
import java.net.URL;
import java.net.UnknownHostException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** Synthetic connections only: no listener, network, Android runtime, or stored credentials. */
public final class ConnectionTestControllerTest {
    public static void main(String[] args) throws Exception {
        try (Harness test = new Harness(1000)) {
            FakeConnection hub = test.add(200);
            test.start("one", "hub", "http://candidate", "synthetic-secret");
            ConnectionTestController.Result result = test.result();
            check(result.ok && result.error == null, "hub success");
            equal("one", result.id); equal("hub", result.source); equal("http://candidate", result.origin);
            equal("http://candidate/api/stats", test.opened.take());
            equal("GET", hub.getRequestMethod());
            equal("application/json", hub.getRequestProperty("Accept"));
            equal("identity", hub.getRequestProperty("Accept-Encoding"));
            equal("Bearer synthetic-secret", hub.getRequestProperty("Authorization"));
            equal("", hub.getRequestProperty("Cookie")); equal("", hub.getRequestProperty("Cookie2"));
            check(!hub.getInstanceFollowRedirects() && !hub.getUseCaches(), "request policy");
            check(result.latencyMs >= 0, "latency");
            FakeConnection archive = test.add(200);
            test.start("two", "archive", "https://archive", "");
            check(test.result().ok, "archive success");
            equal("https://archive/api/session-archive", test.opened.take());
            check(archive.getRequestProperty("Authorization") == null, "no empty bearer");
        }
        for (int status : new int[]{301, 302, 307, 308, 401, 403, 404, 500}) {
            try (Harness test = new Harness(1000)) {
                FakeConnection connection = test.add(status);
                test.start("status", "hub", "http://candidate", "");
                equal(status >= 300 && status < 400 ? "redirect_blocked"
                        : status == 401 || status == 403 ? "unauthorized" : "http_error", test.result().error);
                check(!connection.read, "do not read remote error or redirect body");
            }
        }
        try (Harness test = new Harness(1000)) {
            test.start("empty", "hub", "", "");
            equal("unconfigured", test.result().error);
            check(test.opened.isEmpty(), "unconfigured does not connect");
        }
        try (Harness test = new Harness(1000)) {
            test.add(200).length = NativePolicy.MAX_RESPONSE_BYTES + 1L;
            test.start("size", "hub", "http://candidate", "");
            equal("invalid_response", test.result().error);
        }
        try (Harness test = new Harness(1000)) {
            test.add(200).body = new byte[NativePolicy.MAX_RESPONSE_BYTES + 1];
            test.start("stream-size", "hub", "http://candidate", "");
            equal("invalid_response", test.result().error);
        }
        try (Harness test = new Harness(1000)) {
            test.add(200).body = "invalid".getBytes(StandardCharsets.UTF_8);
            test.start("shape", "hub", "http://candidate", "");
            equal("invalid_response", test.result().error);
        }
        for (Exception failure : new Exception[]{new SocketTimeoutException(), new UnknownHostException(), new ConnectException()}) {
            try (Harness test = new Harness(1000)) {
                test.add(200).failure = failure;
                test.start("failure", "hub", "http://candidate", "");
                equal(failure instanceof SocketTimeoutException ? "timeout"
                        : failure instanceof UnknownHostException ? "host_not_found" : "network_error", test.result().error);
            }
        }
        // ConnectException alone is insufficient evidence for ECONNREFUSED.
        equal("network_error", ConnectionTestController.classifyFailure(new ConnectException("unreachable")));
        equal("timeout", ConnectionTestController.classifyFailure(new IOException(new SocketTimeoutException())));
        equal("host_not_found", ConnectionTestController.classifyFailure(new IOException(new UnknownHostException())));
        try (Harness test = new Harness(150)) {
            FakeConnection blocked = test.add(200); blocked.block = true;
            long started = System.nanoTime();
            test.start("deadline", "hub", "http://candidate", "");
            check(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started) < 100, "start is asynchronous");
            check(blocked.entered.await(1, TimeUnit.SECONDS), "entered request");
            equal("timeout", test.result().error);
            check(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started) < 1000, "overall deadline");
            blocked.release.countDown();
            check(test.results.poll(100, TimeUnit.MILLISECONDS) == null, "no late success after deadline");
        }
        try (Harness test = new Harness(1000)) {
            FakeConnection blocked = test.add(200); blocked.block = true;
            test.start("cancel", "hub", "http://candidate", "");
            check(blocked.entered.await(1, TimeUnit.SECONDS), "entered cancellation request");
            test.controller.cancel();
            equal("cancelled", test.result().error);
            blocked.release.countDown();
            check(test.results.poll(100, TimeUnit.MILLISECONDS) == null, "one callback on cancellation");
        }
        try (Harness test = new Harness(1000)) {
            FakeConnection blocked = test.add(200); blocked.block = true;
            test.start("old", "hub", "http://candidate", "");
            check(blocked.entered.await(1, TimeUnit.SECONDS), "entered replaced request");
            test.add(200);
            test.start("new", "archive", "http://archive", "");
            ConnectionTestController.Result old = test.result();
            equal("old", old.id); equal("cancelled", old.error);
            blocked.release.countDown();
            ConnectionTestController.Result latest = test.result();
            equal("new", latest.id); check(latest.ok, "new request succeeds");
            check(test.results.poll(100, TimeUnit.MILLISECONDS) == null, "old result cannot finish twice");
        }
        try (Harness test = new Harness(1000)) {
            FakeConnection blocked = test.add(200); blocked.block = true;
            test.start("revision", "hub", "http://candidate", "");
            check(blocked.entered.await(1, TimeUnit.SECONDS), "entered revision request");
            test.current.set(false); blocked.release.countDown();
            equal("cancelled", test.result().error);
        }
        try (Harness test = new Harness(1000)) {
            test.current.set(false);
            test.start("background", "hub", "http://candidate", "");
            equal("cancelled", test.result().error);
            check(test.opened.isEmpty(), "no request when background or revision stale");
        }
        try (Harness test = new Harness(1000)) {
            FakeConnection blocked = test.add(200); blocked.block = true;
            test.start("destroy", "hub", "http://candidate", "");
            check(blocked.entered.await(1, TimeUnit.SECONDS), "entered destroyed request");
            test.controller.destroy();
            equal("cancelled", test.result().error);
            blocked.release.countDown();
            test.start("after-destroy", "hub", "http://candidate", "");
            check(test.results.poll(100, TimeUnit.MILLISECONDS) == null, "destroyed controller stays closed");
        }
        System.out.println("Connection test controller checks passed");
    }

    private static final class Harness implements AutoCloseable {
        final BlockingQueue<FakeConnection> pending = new LinkedBlockingQueue<>();
        final BlockingQueue<String> opened = new LinkedBlockingQueue<>();
        final BlockingQueue<ConnectionTestController.Result> results = new LinkedBlockingQueue<>();
        final AtomicBoolean current = new AtomicBoolean(true);
        final List<FakeConnection> all = new ArrayList<>();
        final ConnectionTestController controller;
        Harness(long deadline) {
            controller = new ConnectionTestController(url -> {
                opened.add(url);
                FakeConnection next = pending.poll();
                if (next == null) throw new AssertionError("unexpected network request");
                return next;
            }, (body, source) -> {
                if (!new String(body, StandardCharsets.UTF_8).equals("valid")) throw new IllegalArgumentException();
            }, ConnectionTestController::classifyFailure, deadline);
        }
        FakeConnection add(int status) throws Exception {
            FakeConnection next = new FakeConnection(status); pending.add(next); all.add(next); return next;
        }
        void start(String id, String source, String origin, String secret) {
            controller.start(id, source, origin, secret, current::get, results::add);
        }
        ConnectionTestController.Result result() throws Exception {
            ConnectionTestController.Result result = results.poll(2, TimeUnit.SECONDS);
            if (result == null) throw new AssertionError("missing callback");
            return result;
        }
        public void close() {
            for (FakeConnection connection : all) connection.release.countDown();
            controller.destroy();
        }
    }
    private static final class FakeConnection extends HttpURLConnection {
        final CountDownLatch entered = new CountDownLatch(1), release = new CountDownLatch(1);
        final int status;
        byte[] body = "valid".getBytes(StandardCharsets.UTF_8);
        long length = -1;
        boolean block, read;
        Exception failure;
        FakeConnection(int status) throws Exception { super(new URL("http://synthetic")); this.status = status; }
        @Override public int getResponseCode() throws IOException {
            entered.countDown();
            if (block) {
                boolean released = false;
                // Model a resolver/socket operation that ignores Future.cancel's interruption.
                while (!released) {
                    try { release.await(); released = true; }
                    catch (InterruptedException ignored) { }
                }
            }
            if (failure instanceof IOException) throw (IOException) failure;
            return status;
        }
        @Override public long getContentLengthLong() { return length; }
        @Override public InputStream getInputStream() { read = true; return new ByteArrayInputStream(body); }
        @Override public void disconnect() { }
        @Override public boolean usingProxy() { return false; }
        @Override public void connect() { }
    }
    private static void check(boolean value, String label) { if (!value) throw new AssertionError(label); }
    private static void equal(String expected, String actual) {
        if (!expected.equals(actual)) throw new AssertionError(expected + " != " + actual);
    }
}
