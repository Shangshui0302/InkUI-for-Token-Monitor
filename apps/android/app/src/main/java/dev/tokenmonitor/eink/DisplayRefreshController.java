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
    private long generation;
    private boolean waitingForVisualState;

    DisplayRefreshController(WebView view) {
        this.view = view;
        backend = new OnyxDisplayBackend(Build.MANUFACTURER, Build.MODEL, view, View.class);
    }

    String capabilities() {
        boolean supported = backend.isSupported();
        return "{\"supported\":" + supported + ",\"method\":\""
                + (supported ? OnyxDisplayBackend.METHOD : "system-redraw") + "\"}";
    }

    void request(String reason) {
        if (!policy.request(reason, SystemClock.uptimeMillis())) return;
        // A newer DOM change needs its own fence. Old callbacks cannot consume newer requests.
        generation++;
        waitingForVisualState = false;
        schedule();
    }

    void resume() {
        policy.resume(SystemClock.uptimeMillis());
        schedule();
    }

    void pause() {
        policy.pause();
        cancel();
    }

    void destroy() {
        policy.destroy();
        cancel();
    }

    private void cancel() {
        generation++;
        waitingForVisualState = false;
        handler.removeCallbacks(drain);
    }

    private void schedule() {
        handler.removeCallbacks(drain);
        if (!policy.isActive() || !policy.hasPending() || waitingForVisualState) return;
        handler.postDelayed(drain, policy.delay(SystemClock.uptimeMillis()));
    }

    private void awaitVisualState() {
        if (!policy.isActive() || !policy.hasPending() || waitingForVisualState) return;
        if (!view.isAttachedToWindow() || view.getVisibility() != View.VISIBLE) return;
        waitingForVisualState = true;
        final long ticket = ++generation;
        view.postVisualStateCallback(ticket, new WebView.VisualStateCallback() {
            @Override public void onComplete(long requestId) {
                if (ticket != generation || !policy.isActive()) return;
                waitingForVisualState = false;
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
    }
}
