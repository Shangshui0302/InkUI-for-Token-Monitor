package dev.tokenmonitor.eink;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.URL;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.Semaphore;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.BooleanSupplier;
import java.util.function.Function;

/** Fixed-route native transport. Logical cancellation never waits for a socket or disconnect. */
final class HubHttpClient implements AutoCloseable {
    interface ConnectionFactory { HttpURLConnection open(String url) throws IOException; }
    interface Validator { void validate(byte[] body, String path) throws Exception; }
    static final class Result {
        final byte[] body;
        final String error;
        Result(byte[] body, String error) { this.body = body; this.error = error; }
    }
    private static final long DEADLINE_MS = 10000;
    // Activity recreation must not replenish slots held by an old blocked operation.
    private static final Semaphore READ_SLOTS = new Semaphore(2);
    private static final Semaphore PROBE_SLOTS = new Semaphore(1);
    private final ThreadPoolExecutor workers, cleaners;
    private final Semaphore physicalSlots;
    private final Map<String, Operation> active = new HashMap<>();
    private final ConnectionFactory factory;
    private final Validator validator;
    private final Function<Exception, String> classifier;
    private final long deadlineMs;
    private boolean closed;

    HubHttpClient(Validator validator, Function<Exception, String> classifier) {
        this(url -> (HttpURLConnection) new URL(url).openConnection(Proxy.NO_PROXY),
                validator, classifier, DEADLINE_MS, 2, READ_SLOTS);
    }

    static HubHttpClient probe(Validator validator, Function<Exception, String> classifier) {
        return new HubHttpClient(url -> (HttpURLConnection) new URL(url).openConnection(Proxy.NO_PROXY),
                validator, classifier, DEADLINE_MS, 1, PROBE_SLOTS);
    }

    HubHttpClient(ConnectionFactory factory, Validator validator, Function<Exception, String> classifier,
            long deadlineMs, int concurrency) {
        this(factory, validator, classifier, deadlineMs, concurrency, new Semaphore(concurrency));
    }

    HubHttpClient(ConnectionFactory factory, Validator validator, Function<Exception, String> classifier,
            long deadlineMs, int concurrency, Semaphore physicalSlots) {
        if (deadlineMs <= 0 || concurrency <= 0) throw new IllegalArgumentException();
        this.factory = factory; this.validator = validator; this.classifier = classifier;
        this.deadlineMs = Math.min(DEADLINE_MS, deadlineMs);
        this.physicalSlots = physicalSlots;
        workers = new ThreadPoolExecutor(concurrency, concurrency, 0, TimeUnit.MILLISECONDS,
                new ArrayBlockingQueue<>(concurrency), threads("hub-http"));
        cleaners = new ThreadPoolExecutor(concurrency, concurrency, 0, TimeUnit.MILLISECONDS,
                new ArrayBlockingQueue<>(concurrency), threads("hub-disconnect"));
    }

    Result execute(String requestId, String origin, String secret, String path, BooleanSupplier current) {
        if (!isCurrent(current)) return failure("cancelled");
        try {
            if (requestId == null || requestId.isEmpty() || !NativePolicy.ROUTES.contains(path))
                return failure("invalid_connection");
            origin = NativePolicy.normalizeOrigin(origin);
            secret = NativePolicy.checkSecret(secret);
        } catch (IllegalArgumentException invalid) { return failure("invalid_connection"); }
        if (origin.isEmpty()) return failure("unconfigured");
        Operation operation = new Operation(requestId, origin, secret, path, current, deadlineMs);
        synchronized (this) {
            if (closed) return failure("cancelled");
            if (active.containsKey(requestId)) return failure("invalid_connection");
            if (!physicalSlots.tryAcquire()) return failure("network_error");
            active.put(requestId, operation);
            try { workers.execute(() -> run(operation)); }
            catch (RejectedExecutionException busy) {
                active.remove(requestId);
                physicalSlots.release();
                return failure("network_error");
            }
        }
        try {
            while (operation.result == null) {
                if (!isCurrent(current)) complete(operation, failure("cancelled"));
                long remaining = operation.expires - System.nanoTime();
                if (remaining <= 0) complete(operation, failure("timeout"));
                else operation.finished.await(Math.min(remaining, TimeUnit.MILLISECONDS.toNanos(25)), TimeUnit.NANOSECONDS);
            }
        } catch (InterruptedException interrupted) {
            complete(operation, failure("cancelled"));
            Thread.currentThread().interrupt();
        }
        // Revision changes and cancellation cannot publish an old successful result.
        return isCurrent(current) ? operation.result : failure("cancelled");
    }

    void cancel(String requestId) {
        Operation operation;
        synchronized (this) { operation = active.get(requestId); }
        if (operation != null) complete(operation, failure("cancelled"));
    }

    void cancelAll() {
        Operation[] operations;
        synchronized (this) { operations = active.values().toArray(new Operation[0]); }
        for (Operation operation : operations) complete(operation, failure("cancelled"));
    }

    @Override public void close() {
        synchronized (this) { closed = true; }
        cancelAll();
        // A task admitted during a worker's final handoff may not have started yet.
        // Drained tasks opened no socket, so return their shared permits here.
        for (Runnable pending : workers.shutdownNow()) physicalSlots.release();
        cleaners.shutdown();
    }

    private void run(Operation operation) {
        operation.thread = Thread.currentThread();
        Result success = null;
        try {
            if (!usable(operation)) return;
            HttpURLConnection connection = factory.open(operation.origin + operation.path);
            operation.connection = connection;
            if (!usable(operation)) return;
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout((int) Math.min(8000, deadlineMs));
            connection.setReadTimeout((int) Math.min(8000, deadlineMs));
            connection.setUseCaches(false);
            connection.setRequestMethod("GET");
            connection.setRequestProperty("Accept", "application/json");
            connection.setRequestProperty("Accept-Encoding", "identity");
            connection.setRequestProperty("Cookie", "");
            connection.setRequestProperty("Cookie2", "");
            if (!operation.secret.isEmpty()) connection.setRequestProperty("Authorization", "Bearer " + operation.secret);
            if (!usable(operation)) return;
            int status = connection.getResponseCode();
            if (!usable(operation)) return;
            if (status >= 300 && status <= 399) complete(operation, failure("redirect_blocked"));
            else if (status == 401 || status == 403) complete(operation, failure("unauthorized"));
            else if (status != 200) complete(operation, failure("http_error"));
            else if (connection.getContentLengthLong() > NativePolicy.MAX_RESPONSE_BYTES) complete(operation, failure("invalid_response"));
            else {
                byte[] body;
                try (InputStream input = connection.getInputStream()) { body = readBody(operation, input); }
                if (body == null || !usable(operation)) return;
                try { validator.validate(body, operation.path); }
                catch (Exception invalid) { complete(operation, failure("invalid_response")); return; }
                if (usable(operation)) success = new Result(body, null);
            }
        } catch (Exception failure) {
            String error = "response_limit".equals(failure.getMessage()) ? "invalid_response" : classifier.apply(failure);
            complete(operation, failure(error == null ? "network_error" : error));
        } finally {
            cleanup(operation);
            // A stuck disconnect retains this physical slot. Repeated attempts cannot accumulate sockets.
            boolean interrupted = false;
            while (operation.connection != null && operation.cleaned.getCount() != 0) {
                cleanup(operation);
                try { operation.cleaned.await(25, TimeUnit.MILLISECONDS); }
                catch (InterruptedException ignored) { interrupted = true; }
            }
            operation.thread = null;
            physicalSlots.release();
            if (success != null) complete(operation, success);
            else if (operation.result == null) complete(operation, failure("cancelled"));
            if (interrupted) Thread.currentThread().interrupt();
        }
    }

    private byte[] readBody(Operation operation, InputStream input) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        byte[] buffer = new byte[16384];
        while (usable(operation)) {
            int count = input.read(buffer);
            if (!usable(operation)) return null;
            if (count == -1) return output.toByteArray();
            if (count > NativePolicy.MAX_RESPONSE_BYTES - output.size()) throw new IOException("response_limit");
            output.write(buffer, 0, count);
        }
        return null;
    }

    private boolean usable(Operation operation) {
        if (operation.result != null) return false;
        if (!isCurrent(operation.current)) { complete(operation, failure("cancelled")); return false; }
        if (System.nanoTime() >= operation.expires) { complete(operation, failure("timeout")); return false; }
        return true;
    }

    private void complete(Operation operation, Result result) {
        if (result.error == null) {
            if (!isCurrent(operation.current)) result = failure("cancelled");
            else if (System.nanoTime() >= operation.expires) result = failure("timeout");
        }
        synchronized (operation) {
            if (operation.result != null) return;
            operation.result = result;
        }
        synchronized (this) { active.remove(operation.id, operation); }
        operation.finished.countDown();
        if (result.error != null) {
            Thread thread = operation.thread;
            if (thread != null && thread != Thread.currentThread()) thread.interrupt();
            cleanup(operation);
        }
    }

    private void cleanup(Operation operation) {
        synchronized (operation) {
            if (operation.connection == null || operation.cleanupStarted) return;
            operation.cleanupStarted = true;
        }
        Runnable disconnect = () -> {
            try { operation.connection.disconnect(); }
            finally { operation.cleaned.countDown(); }
        };
        try { cleaners.execute(disconnect); }
        catch (RejectedExecutionException stopped) {
            // Only a physical worker may perform fallback cleanup after close.
            if (Thread.currentThread() == operation.thread) disconnect.run();
            else synchronized (operation) { operation.cleanupStarted = false; }
        }
    }

    private static boolean isCurrent(BooleanSupplier current) {
        try { return current != null && current.getAsBoolean(); }
        catch (RuntimeException invalid) { return false; }
    }
    private static Result failure(String error) { return new Result(null, error); }
    private static ThreadFactory threads(String name) {
        AtomicInteger ids = new AtomicInteger();
        return task -> {
            Thread thread = new Thread(task, name + "-" + ids.incrementAndGet());
            thread.setDaemon(true); return thread;
        };
    }
    private static final class Operation {
        final String id, origin, secret, path;
        final BooleanSupplier current;
        final long expires;
        final CountDownLatch finished = new CountDownLatch(1), cleaned = new CountDownLatch(1);
        volatile HttpURLConnection connection;
        volatile Thread thread;
        volatile Result result;
        boolean cleanupStarted;
        Operation(String id, String origin, String secret, String path, BooleanSupplier current, long deadlineMs) {
            this.id = id; this.origin = origin; this.secret = secret; this.path = path; this.current = current;
            this.expires = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(deadlineMs);
        }
    }
}
