package dev.tokenmonitor.eink;

import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.SocketTimeoutException;
import java.net.UnknownHostException;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.Future;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.Function;

/** One asynchronous candidate probe using the same transport policy as normal Hub reads. */
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
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS,
            new ArrayBlockingQueue<>(1), task -> {
                Thread thread = new Thread(task, "hub-probe"); thread.setDaemon(true); return thread;
            });
    private final HubHttpClient client;
    private Test active;
    private long sequence;
    private boolean closed;

    ConnectionTestController(Validator validator, Function<Exception, String> classify) {
        client = HubHttpClient.probe((body, path) -> validator.validate(body, "hub"), classify);
    }
    ConnectionTestController(ConnectionFactory factory, Validator validator,
            Function<Exception, String> classify, long deadlineMs) {
        client = new HubHttpClient(factory::open, (body, path) -> validator.validate(body, "hub"),
                classify, deadlineMs, 1);
    }

    synchronized void start(String id, String source, String origin, String secret,
            BooleanSupplier current, Consumer<Result> callback) {
        cancel();
        if (closed) return;
        if (!"hub".equals(source)) {
            callback.accept(new Result(id, "hub", "", "invalid_connection", 0));
            return;
        }
        Test test = new Test(id, source, origin, "probe-" + (++sequence), current, callback);
        active = test;
        try {
            test.future = worker.submit(() -> {
                HubHttpClient.Result result = client.execute(test.requestId, origin, secret, "/api/stats",
                        () -> isActive(test) && test.current.getAsBoolean());
                finish(test, result.error);
            });
        } catch (RejectedExecutionException busy) { finish(test, "network_error"); }
    }

    synchronized void cancel() {
        if (active != null) finish(active, "cancelled");
    }

    synchronized void destroy() {
        cancel(); closed = true;
        worker.shutdownNow(); client.close();
    }

    private synchronized boolean isActive(Test test) { return active == test && !closed; }

    private synchronized void finish(Test test, String error) {
        if (active != test) return;
        active = null;
        if (error != null) {
            client.cancel(test.requestId);
            if (test.future != null) {
                test.future.cancel(true);
                worker.remove((Runnable) test.future);
            }
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
        final String id, source, origin, requestId;
        final BooleanSupplier current;
        final Consumer<Result> callback;
        final long started = System.nanoTime();
        Future<?> future;
        Test(String id, String source, String origin, String requestId,
                BooleanSupplier current, Consumer<Result> callback) {
            this.id = id; this.source = source; this.origin = origin; this.requestId = requestId;
            this.current = current; this.callback = callback;
        }
    }
}
