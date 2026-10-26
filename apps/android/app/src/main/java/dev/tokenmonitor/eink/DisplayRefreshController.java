package dev.tokenmonitor.eink;

import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;
import android.webkit.WebView;

/** Main-thread coordinator: refresh only after Chromium has committed the requested DOM state. */
final class DisplayRefreshController {
    private final WebView view;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
    private final OnyxDisplayBackend backend;
    private final Runnable drain = this::awaitVisualState;
    private final DisplayRefreshPolicy.VisualFence fence = new DisplayRefreshPolicy.VisualFence();
    private Runnable visualTimeout;
    private final View.OnAttachStateChangeListener attachment = new View.OnAttachStateChangeListener() {
        @Override public void onViewAttachedToWindow(View ignored) { fence.resetRetries(); schedule(); }
        @Override public void onViewDetachedFromWindow(View ignored) { cancel(); }
    };

    DisplayRefreshController(WebView view) {
        this.view = view;
        backend = new OnyxDisplayBackend(Build.MANUFACTURER, Build.MODEL, view, View.class);
        view.addOnAttachStateChangeListener(attachment);
    }

    String capabilities() {
        boolean supported = backend.isSupported();
        return "{\"supported\":" + supported + ",\"method\":\""
                + (supported ? OnyxDisplayBackend.METHOD : "system-redraw") + "\"}";
    }

    void request(String reason) {
        if (!policy.request(reason, SystemClock.uptimeMillis())) return;
        // A newer DOM change needs its own fence. Old callbacks cannot consume newer requests.
        cancel();
        fence.resetRetries();
        schedule();
    }

    void resume() {
        cancel();
        policy.resume(SystemClock.uptimeMillis());
        fence.resetRetries();
        schedule();
    }

    void pause() {
        policy.pause();
        cancel();
    }

    void destroy() {
        policy.destroy();
        cancel();
        view.removeOnAttachStateChangeListener(attachment);
    }

    private void cancel() {
        fence.cancel();
        clearVisualTimeout();
        handler.removeCallbacks(drain);
    }

    private void schedule() {
        handler.removeCallbacks(drain);
        if (!policy.isActive() || !policy.hasPending() || fence.isWaiting()) return;
        handler.postDelayed(drain, policy.delay(SystemClock.uptimeMillis()));
    }

    private void awaitVisualState() {
        if (!policy.isActive() || !policy.hasPending() || fence.isWaiting()) return;
        if (!view.isAttachedToWindow() || view.getVisibility() != View.VISIBLE) return;
        final long ticket = fence.begin();
        visualTimeout = () -> {
            if (!fence.complete(ticket)) return;
            visualTimeout = null;
            retryVisualState();
        };
        handler.postDelayed(visualTimeout, DisplayRefreshPolicy.VisualFence.TIMEOUT_MS);
        try {
            view.postVisualStateCallback(ticket, new WebView.VisualStateCallback() {
                @Override public void onComplete(long requestId) {
                    if (!fence.complete(ticket)) return;
                    clearVisualTimeout();
                    if (!policy.isActive()) return;
                    DisplayRefreshPolicy.Update update = policy.take(SystemClock.uptimeMillis());
                    if (update != DisplayRefreshPolicy.Update.NONE) {
                        boolean wasSupported = backend.isSupported();
                        if (!backend.refresh(update == DisplayRefreshPolicy.Update.FULL)) view.invalidate();
                        if (wasSupported && !backend.isSupported()) {
                            view.evaluateJavascript("window.dispatchEvent(new Event('tokenmonitor:display-capabilities'))", null);
                        }
                    }
                    schedule();
                }
            });
        } catch (RuntimeException ignored) {
            // A destroyed/unavailable WebView must not leave the queue permanently waiting.
            // Preserve pending intent for the next lifecycle or visible-change request.
            clearVisualTimeout();
            retryVisualState();
        }
    }

    private void retryVisualState() {
        fence.cancel();
        // Retry a transient renderer failure once. After the second failure retain
        // pending intent without polling; a real DOM request, attach, or resume resets
        // the budget. No timeout dispatches hardware work before a visual commit.
        if (fence.retry()) schedule();
    }

    private void clearVisualTimeout() {
        if (visualTimeout != null) handler.removeCallbacks(visualTimeout);
        visualTimeout = null;
    }
}
