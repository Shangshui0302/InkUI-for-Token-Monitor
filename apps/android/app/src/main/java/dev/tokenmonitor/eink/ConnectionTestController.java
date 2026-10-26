package dev.tokenmonitor.eink;

import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.SocketTimeoutException;
import java.net.URL;
import java.net.UnknownHostException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.Function;

/** One asynchronous, bounded, read-only candidate probe. No Android UI or persistent state. */
final class ConnectionTestController {
    interface ConnectionFactory { HttpURLConnection open(String url) throws IOException; }
    interface Validator { void validate(byte[] body, String source) throws Exception; }
    static final class Result {
        final String id, source, origin, error;
        final boolean ok;
        final long latencyMs;
        Result(String id, String source, String origin, String error, long latencyMs) {
            this.id = id; this.source = source; this.origin = origin; this.error = error;
            this.ok = error == null; this.latencyMs = latencyMs;
        }
    }
    private static final long DEADLINE_MS = 10000;
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final ScheduledThreadPoolExecutor timer = new ScheduledThreadPoolExecutor(2);
    private final ConnectionFactory factory;
    private final Validator validator;
    private final Function<Exception, String> classify;
    private final long deadlineMs;
    private Test active;
    private boolean closed;

    ConnectionTestController(Validator validator, Function<Exception, String> classify) {
        this(url -> (HttpURLConnection) new URL(url).openConnection(Proxy.NO_PROXY), validator, classify, DEADLINE_MS);
    }
    ConnectionTestController(ConnectionFactory factory, Validator validator,
            Function<Exception, String> classify, long deadlineMs) {
        this.factory = factory; this.validator = validator; this.classify = classify;
        this.deadlineMs = deadlineMs;
        timer.setRemoveOnCancelPolicy(true);
    }

    synchronized void start(String id, String source, String origin, String secret,
            BooleanSupplier current, Consumer<Result> callback) {
        cancel();
        if (closed) return;
        if (!"hub".equals(source)) {
            callback.accept(new Result(id, "hub", "", "invalid_connection", 0));
            return;
        }
        Test test = new Test(id, source, origin, secret, current, callback);
        active = test;
        test.deadline = timer.schedule(() -> finish(test, "timeout"), deadlineMs, TimeUnit.MILLISECONDS);
        test.future = worker.submit(() -> execute(test));
    }

    synchronized void cancel() {
        if (active != null) finish(active, "cancelled");
    }

    synchronized void destroy() {
        cancel(); closed = true;
        worker.shutdownNow();
        // Allow already queued socket cleanup to run, without waiting on the activity thread.
        timer.shutdown();
    }

    private void execute(Test test) {
        HttpURLConnection connection = null;
        String error;
        try {
            if (!isActive(test)) return;
            if (!test.current.getAsBoolean()) { finish(test, "cancelled"); return; }
            if (test.origin.isEmpty()) { finish(test, "unconfigured"); return; }
            connection = factory.open(test.origin + "/api/stats");
            synchronized (this) {
                if (active != test) { connection.disconnect(); return; }
                test.connection = connection;
            }
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout((int) Math.min(8000, deadlineMs));
            connection.setReadTimeout((int) Math.min(8000, deadlineMs));
            connection.setUseCaches(false);
            connection.setRequestMethod("GET");
            connection.setRequestProperty("Accept", "application/json");
            connection.setRequestProperty("Accept-Encoding", "identity");
            // Never import the WebView's cookie store into this native request.
            connection.setRequestProperty("Cookie", "");
            connection.setRequestProperty("Cookie2", "");
            if (!test.secret.isEmpty()) connection.setRequestProperty("Authorization", "Bearer " + test.secret);
            if (!isActive(test) || !test.current.getAsBoolean()) { finish(test, "cancelled"); return; }
            int status = connection.getResponseCode();
            if (status >= 300 && status <= 399) error = "redirect_blocked";
            else if (status == 401 || status == 403) error = "unauthorized";
            else if (status != 200) error = "http_error";
            else if (connection.getContentLengthLong() > NativePolicy.MAX_RESPONSE_BYTES) error = "invalid_response";
            else {
                byte[] body;
                try (InputStream in = connection.getInputStream()) { body = NativePolicy.readLimited(in); }
                try { validator.validate(body, test.source); }
                catch (Exception invalid) { finish(test, "invalid_response"); return; }
                error = null;
            }
        } catch (Exception failure) {
            error = "response_limit".equals(failure.getMessage()) ? "invalid_response" : classify.apply(failure);
        } finally {
            if (connection != null) connection.disconnect();
        }
        finish(test, error);
    }

    private synchronized boolean isActive(Test test) { return active == test && !closed; }

    private synchronized void finish(Test test, String error) {
        if (active != test) return;
        active = null;
        if (test.deadline != null) test.deadline.cancel(false);
        if (error != null) {
            if (test.future != null) test.future.cancel(true);
            if (test.connection != null) timer.execute(test.connection::disconnect);
        }
        if (!test.current.getAsBoolean()) error = "cancelled";
        test.callback.accept(new Result(test.id, test.source, test.origin, error,
                TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - test.started)));
    }

    static String classifyFailure(Exception failure) {
        for (Throwable cause = failure; cause != null; cause = cause.getCause()) {
            if (cause instanceof SocketTimeoutException) return "timeout";
            if (cause instanceof UnknownHostException) return "host_not_found";
        }
        return "network_error";
    }

    private static final class Test {
        final String id, source, origin, secret;
        final BooleanSupplier current;
        final Consumer<Result> callback;
        final long started = System.nanoTime();
        HttpURLConnection connection;
        Future<?> future;
        ScheduledFuture<?> deadline;
        Test(String id, String source, String origin, String secret,
                BooleanSupplier current, Consumer<Result> callback) {
            this.id = id; this.source = source; this.origin = origin; this.secret = secret;
            this.current = current; this.callback = callback;
        }
    }
}
