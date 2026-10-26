package dev.tokenmonitor.eink;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Rect;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.InputMethodManager;
import android.webkit.WebView;
import android.webkit.WebViewRenderProcess;
import org.json.JSONArray;
import org.json.JSONObject;
import java.lang.reflect.Field;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

/** Framework-only, opt-in debug checks. No AndroidX, JUnit, UI Automator or remote Hub. */
public final class EngineeringInstrumentation extends Instrumentation {
    private static final long TIMEOUT_MS = 15000;
    private static final String CLASS = EngineeringInstrumentation.class.getName();
    private Bundle arguments;
    private MainActivity activity;
    private int failures;
    private int total;
    private int current;
    private final StringBuilder transcript = new StringBuilder();

    @Override public void onCreate(Bundle args) {
        super.onCreate(args);
        arguments = args == null ? new Bundle() : new Bundle(args);
        start();
    }

    @Override public void onStart() {
        super.onStart();
        List<Check> checks = new ArrayList<>();
        String suite = arguments.getString("suite", "all");
        if ("all".equals(suite) || "storage".equals(suite))
            checks.add(new Check("encrypted_storage_recovery", () -> ConnectionStoreDeviceChecks.run(getTargetContext())));
        if ("all".equals(suite) || "lifecycle".equals(suite))
            checks.add(new Check("activity_recreation_and_per_hub_persistence", this::recreation));
        if ("all".equals(suite) || "webview".equals(suite)) {
            checks.add(new Check("fullscreen_keyboard_fixed_viewport", this::keyboard));
            checks.add(new Check("renderer_recovery_budget_and_manual_retry", this::renderer));
        }
        if ("all".equals(suite) || "network".equals(suite))
            checks.add(new Check("framework_network_callback_lifecycle", this::network));
        if ("all".equals(suite) || "http".equals(suite))
            checks.add(new Check("loopback_proxy_cancel_and_visible_reading_restore", this::http));
        if ("all".equals(suite) || "recovery".equals(suite))
            checks.add(new Check("native_recovery_requires_successful_framework_probe", this::recovery));
        if (checks.isEmpty()) checks.add(new Check("valid_suite_required", () -> {
            throw new AssertionError("suite must be all, storage, lifecycle, webview, network, http or recovery");
        }));
        total = checks.size();
        for (Check check : checks) {
            current++;
            status(check.name, 1, null);
            Throwable failed = null;
            try {
                require(getTargetContext().getPackageName().endsWith(".debug"), "refusing a production application");
                require(Build.VERSION.SDK_INT >= 29, "API 29 or later required");
                check.body.run();
            } catch (Throwable failure) {
                failed = failure;
            } finally {
                try { closeActivity(); }
                catch (Throwable cleanup) {
                    if (failed == null) failed = cleanup;
                    else failed.addSuppressed(cleanup);
                }
            }
            if (failed == null) {
                transcript.append("PASS ").append(check.name).append('\n');
                status(check.name, 0, null);
            } else {
                failures++;
                String detail = safeFailure(failed);
                transcript.append("FAIL ").append(check.name).append(": ").append(detail).append('\n');
                status(check.name, -2, detail);
            }
        }
        Bundle result = new Bundle();
        String summary = failures == 0 ? "OK (" + total + " tests)" : "FAILURES!!! Tests run: " + total + ", Failures: " + failures;
        result.putString("stream", "\n" + transcript + "\n" + summary + "\n");
        result.putInt("tests", total);
        result.putInt("failures", failures);
        finish(failures == 0 ? Activity.RESULT_OK : Activity.RESULT_CANCELED, result);
    }

    private void status(String name, int code, String failure) {
        Bundle value = new Bundle();
        value.putString("class", CLASS);
        value.putString("test", name);
        value.putInt("numtests", total);
        value.putInt("current", current);
        value.putString("id", "EngineeringInstrumentation");
        value.putString("stream", failure == null ? (code == 0 ? "." : "\n" + name + ": ") : "\n" + failure + "\n");
        if (failure != null) value.putString("stack", failure);
        sendStatus(code, value);
    }

    private void launch() throws Exception {
        // Even the debug application's existing connection is left alone. In particular,
        // do not launch a WebView that could issue GETs to a configured personal Hub.
        ConnectionStore store = new ConnectionStore(getTargetContext());
        require("unconfigured".equals(store.storageState()), "activity checks require an unconfigured debug installation");
        Intent intent = new Intent(getTargetContext(), MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
        Activity launched = startActivitySync(intent);
        require(launched instanceof MainActivity, "debug MainActivity did not launch");
        activity = (MainActivity) launched;
        ready(webView());
    }

    private void recreation() throws Exception {
        launch();
        String origin = "https://engineering-" + UUID.randomUUID() + ".invalid";
        String key = "inkui.provider-order.v1:" + origin;
        String readingKey = "inkui.reading-state.v1:" + origin;
        String themeKey = "token-monitor-eink-theme-v1";
        String intervalKey = "token-monitor-eink-interval-v1";
        String previous = js(webView(), "JSON.stringify([localStorage.getItem(" + q(themeKey) + "),localStorage.getItem(" + q(intervalKey) + ")])");
        JSONArray prior = new JSONArray(new JSONArray("[" + previous + "]").getString(0));
        try {
            WebView first = webView();
            // Use the actual packaged order store with a unique synthetic Hub, never the native connection.
            js(first, "window.__engineeringOrder=null;" + importing("provider-order.mjs", "const s=m.createProviderOrderStore(localStorage);window.__engineeringOrder=s.save(" + q(origin) + ",['engineering-b','engineering-a']);"));
            awaitModule(first, "window.__engineeringOrder===true", "packaged provider order store did not persist");
            js(first, "window.__engineeringReading=null;" + importing("ui-state.mjs", "window.__engineeringReading=m.createReadingStateStore(localStorage).save(" + q(origin) + ",{page:'activity',period:'month',activityKind:'day',index:2});"));
            awaitModule(first, "window.__engineeringReading===true", "packaged reading state store did not persist");
            js(first, "document.querySelector('[data-theme-choice=\"color\"]').click();document.getElementById('interval').value='300';document.getElementById('interval').dispatchEvent(new Event('change'));true");
            require("\"color\"".equals(js(first, "document.documentElement.dataset.theme")), "theme handler was not applied");
            ActivityMonitor monitor = addMonitor(MainActivity.class.getName(), null, false);
            MainActivity old = activity;
            try {
                runOnMainSync(old::recreate);
                Activity replacement = waitForMonitorWithTimeout(monitor, TIMEOUT_MS);
                require(replacement instanceof MainActivity && replacement != old, "framework did not recreate Activity");
                activity = (MainActivity) replacement;
            } finally { removeMonitor(monitor); }
            waitForIdleSync();
            require(onMain(old::isDestroyed), "old Activity remained alive after recreation");
            requireStopped(old);
            WebView second = webView();
            require(second != first, "recreation reused the old WebView");
            ready(second);
            require("\"color\"".equals(js(second, "document.documentElement.dataset.theme")), "actual UI theme did not survive recreation");
            require("\"300\"".equals(js(second, "document.getElementById('interval').value")), "actual refresh preference did not survive recreation");
            require("[\"engineering-b\",\"engineering-a\"]".equals(js(second, "JSON.parse(localStorage.getItem(" + q(key) + "))")), "per-Hub order did not survive WebView recreation");
            js(second, "window.__engineeringOrder=null;" + importing("provider-order.mjs", "window.__engineeringOrder=JSON.stringify(m.createProviderOrderStore(localStorage).load(" + q(origin) + "));"));
            awaitModule(second, "window.__engineeringOrder==='[\"engineering-b\",\"engineering-a\"]'", "packaged store did not read persisted Hub order");
            js(second, "window.__engineeringReading=null;" + importing("ui-state.mjs", "const s=m.createReadingStateStore(localStorage).load(" + q(origin) + ");window.__engineeringReading=s.page==='activity'&&s.period==='month'&&s.activityKind==='day'&&s.index===2;"));
            awaitModule(second, "window.__engineeringReading===true", "packaged store did not read persisted Hub reading state");
            assertViewport(second, false);
        } finally {
            if (activity != null && webView() != null) {
                js(webView(), "localStorage.removeItem(" + q(key) + ");localStorage.removeItem(" + q(readingKey) + ");" + restore(themeKey, prior.opt(0)) + restore(intervalKey, prior.opt(1)) + "true");
            }
        }
    }

    private void renderer() throws Exception {
        launch();
        MainActivity owner = activity;
        WebView first = webView();
        terminate(first);
        await(() -> webView() != null && webView() != first, "first renderer loss did not replace WebView");
        WebView recovered = webView();
        ready(recovered);
        require(activity == owner && !onMain(owner::isDestroyed), "renderer loss destroyed the Activity");
        require(onMain(() -> owner.getWindow().getDecorView().findViewWithTag("inkui-recovery") == null), "automatic recovery left fallback visible");
        assertViewport(recovered, false);
        // A ready notification must not replenish the automatic retry budget.
        terminate(recovered);
        await(() -> onMain(() -> owner.getWindow().getDecorView().findViewWithTag("inkui-recovery") != null), "repeated renderer loss did not enter bounded native fallback");
        require(webView() == null, "native fallback retained an unusable WebView");
        SystemClock.sleep(600);
        require(webView() == null, "fallback entered an automatic retry loop");
        View retry = onMain(() -> owner.getWindow().getDecorView().findViewWithTag("inkui-retry"));
        require(retry != null && onMain(retry::isShown), "native fallback has no visible retry action");
        runOnMainSync(retry::performClick);
        await(() -> webView() != null, "explicit native retry did not create WebView");
        WebView retried = webView();
        require(retried != recovered && retried != first, "manual retry reused a destroyed WebView");
        ready(retried);
        require(!onMain(owner::isDestroyed), "manual retry finished Activity");
        assertViewport(retried, false);
    }

    private void keyboard() throws Exception {
        SharedPreferences preferences = getTargetContext().getSharedPreferences("device_ui", Context.MODE_PRIVATE);
        boolean hadPreference = preferences.contains("fullscreen");
        boolean previous = preferences.getBoolean("fullscreen", true);
        try {
            launch();
            WebView view = webView();
            setFullscreen(view, true);
            exerciseKeyboard(view);
            setFullscreen(view, false);
            exerciseKeyboard(view);
        } finally {
            SharedPreferences.Editor editor = preferences.edit();
            if (hadPreference) editor.putBoolean("fullscreen", previous); else editor.remove("fullscreen");
            require(editor.commit(), "fullscreen fixture preference was not restored");
        }
    }

    private void setFullscreen(WebView view, boolean enabled) throws Exception {
        js(view, "TokenMonitorAndroid.setFullscreen(" + enabled + ");true");
        awaitJs(view, "document.documentElement.dataset.fullscreen==='" + enabled + "'", "fullscreen state did not reach UI");
        await(() -> onMain(() -> {
            View decor = activity.getWindow().getDecorView();
            if (Build.VERSION.SDK_INT >= 30) {
                WindowInsets insets = decor.getRootWindowInsets();
                return insets != null && insets.isVisible(WindowInsets.Type.statusBars()) != enabled;
            }
            return ((decor.getSystemUiVisibility() & View.SYSTEM_UI_FLAG_FULLSCREEN) != 0) == enabled;
        }), "system bar visibility did not follow fullscreen preference");
        require(onMain(() -> (activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_FULLSCREEN) == 0), "fullscreen flags prevent keyboard resize");
        assertViewport(view, false);
    }

    private void exerciseKeyboard(WebView view) throws Exception {
        js(view, "document.getElementById('settings-open').click();document.getElementById('hub-edit').click();true");
        awaitJs(view, "!document.getElementById('connection-editor').hidden", "connection editor did not open");
        await(() -> onMain(() -> view.isAttachedToWindow() && view.isShown() && view.hasWindowFocus()
                && view.getWindowToken() != null && view.isLaidOut() && !view.isLayoutRequested()
                && view.getWidth() > 0 && view.getHeight() > 0), "keyboard target window or native layout is not ready");
        // Wait for Chromium's editor resize and the native inset layout before
        // measuring the input. A previous fullscreen transition can still be settling.
        CountDownLatch frames = new CountDownLatch(1);
        runOnMainSync(() -> view.postOnAnimation(() -> view.postOnAnimation(frames::countDown)));
        require(frames.await(5, TimeUnit.SECONDS), "keyboard target layout frames did not complete");
        awaitJs(view, "(()=>{const e=document.getElementById('connection-url'),r=e.getBoundingClientRect();return !e.disabled&&r.width>0&&r.height>0&&r.left>=0&&r.top>=0&&r.right<=innerWidth&&r.bottom<=innerHeight})()", "keyboard input is not inside the settled viewport");
        int baseline = onMain(view::getHeight);
        Rect originalFrame = frame();
        JSONArray rect = new JSONArray(js(view, "(()=>{const r=document.getElementById('connection-url').getBoundingClientRect();return [r.left+r.width/2,r.top+r.height/2,innerWidth]})()"));
        double scale = onMain(view::getWidth) / rect.getDouble(2);
        float x = (float) (rect.getDouble(0) * scale);
        float y = (float) (rect.getDouble(1) * scale);
        require(onMain(() -> x >= 0 && y >= 0 && x < view.getWidth() && y < view.getHeight()), "editor input is outside native viewport");
        onMain(() -> {
            require(view.requestFocus() && view.hasWindowFocus(), "keyboard target did not obtain native view and window focus");
            long now = SystemClock.uptimeMillis();
            MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, x, y, 0);
            MotionEvent up = MotionEvent.obtain(now, now, MotionEvent.ACTION_UP, x, y, 0);
            down.setSource(InputDevice.SOURCE_TOUCHSCREEN);
            up.setSource(InputDevice.SOURCE_TOUCHSCREEN);
            // Exercise the known WebView's native touch path in local coordinates.
            // API 36 CI rejected global injection before WebView touch delivery;
            // this avoids that separate system input-target ownership boundary.
            try {
                require(view.dispatchTouchEvent(down), "WebView rejected keyboard touch-down");
                require(view.dispatchTouchEvent(up), "WebView rejected keyboard touch-up");
            } finally { down.recycle(); up.recycle(); }
            return null;
        });
        awaitJs(view, "document.activeElement?.id==='connection-url'", "native touch did not focus the actual editor input");
        await(() -> onMain(() -> view.hasFocus() && view.hasWindowFocus()), "keyboard target lost native view or window focus");
        runOnMainSync(() -> {
            InputMethodManager ime = activity.getSystemService(InputMethodManager.class);
            ime.showSoftInput(view, InputMethodManager.SHOW_IMPLICIT);
        });
        await(() -> onMain(() -> {
            WindowInsets insets = view.getRootWindowInsets();
            return Build.VERSION.SDK_INT >= 30 ? insets != null && insets.isVisible(WindowInsets.Type.ime())
                    : frame().height() < originalFrame.height() - 80;
        }), "soft keyboard did not become visible; fixture requires an enabled software IME");
        await(() -> onMain(view::getHeight) < baseline - 80, "visible IME did not shrink WebView container");
        assertViewport(view, true);
        js(view, "document.activeElement.blur();true");
        runOnMainSync(() -> activity.getSystemService(InputMethodManager.class).hideSoftInputFromWindow(view.getWindowToken(), 0));
        await(() -> Math.abs(onMain(view::getHeight) - baseline) <= 2, "keyboard dismissal left ghost viewport padding");
        assertViewport(view, false);
        js(view, "document.getElementById('connection-cancel').click();document.getElementById('settings-close').click();true");
    }

    private void network() throws Exception {
        ConnectivityManager manager = getTargetContext().getSystemService(ConnectivityManager.class);
        require(manager != null && manager.getAllNetworks().length > 0, "network fixture requires at least one connected framework network");
        AtomicInteger events = new AtomicInteger();
        DeviceStatusController controller = onMain(() -> new DeviceStatusController(getTargetContext(), true, json -> events.incrementAndGet()));
        try {
            onMain(() -> {
                controller.pageReady();
                controller.start();
                require(field(controller, "networkCallback") != null, "framework network callback registration failed");
                // Clear the synchronous initial scan before the main looper can deliver
                // registerNetworkCallback's real initial events. Only callbacks refill it.
                ((Map<?, ?>) field(controller, "links")).clear();
                ((Map<?, ?>) field(controller, "routes")).clear();
                return null;
            });
            await(() -> onMain(() -> !((Map<?, ?>) field(controller, "links")).isEmpty()
                    && !((Map<?, ?>) field(controller, "routes")).isEmpty()), "registered callback received no real capabilities/link-properties events");
            require(events.get() > 0, "real network callbacks published no status");
            Object callback = onMain(() -> field(controller, "networkCallback"));
            runOnMainSync(controller::start);
            require(onMain(() -> field(controller, "networkCallback") == callback), "start registered duplicate callback");
            runOnMainSync(controller::stop);
            require(onMain(() -> !((Boolean) field(controller, "active")) && !((Boolean) field(controller, "receiverRegistered"))
                    && field(controller, "networkCallback") == null), "stop retained framework subscriptions");
            int stoppedEvents = events.get();
            String stoppedSnapshot = controller.snapshotJson();
            // A native callback already queued at stop must also be harmless. This
            // deliberately invokes that retained callback, separate from real delivery above.
            Network existing = manager.getAllNetworks()[0];
            NetworkCapabilities caps = manager.getNetworkCapabilities(existing);
            LinkProperties properties = manager.getLinkProperties(existing);
            onMain(() -> {
                ConnectivityManager.NetworkCallback stale = (ConnectivityManager.NetworkCallback) callback;
                stale.onAvailable(existing);
                if (caps != null) stale.onCapabilitiesChanged(existing, caps);
                if (properties != null) stale.onLinkPropertiesChanged(existing, properties);
                stale.onLost(existing);
                return null;
            });
            waitForIdleSync();
            require(events.get() == stoppedEvents && stoppedSnapshot.equals(controller.snapshotJson()), "stale callback changed stopped controller");
            runOnMainSync(controller::stop);
            runOnMainSync(controller::start);
            require(onMain(() -> field(controller, "networkCallback") != null && field(controller, "networkCallback") != callback), "restart did not register fresh callback generation");
        } finally { runOnMainSync(controller::stop); }
    }

    private void http() throws Exception {
        launch();
        try (LoopbackHub hub = new LoopbackHub()) {
            ConnectionStore memory = new ConnectionStore(new ConnectionStore.Storage() {
                @Override public ConnectionStore.StoredRecord read() {
                    return new ConnectionStore.StoredRecord(new ConnectionStore.Config(hub.origin(), "engineering-synthetic-secret", 1), false);
                }
                @Override public void write(ConnectionStore.Config config) { throw new AssertionError("HTTP fixture must never save a connection"); }
            });
            onMain(() -> { Field slot = MainActivity.class.getDeclaredField("connections"); slot.setAccessible(true); slot.set(activity, memory); return null; });
            WebView initial = webView();
            String readingKey = "inkui.reading-state.v1:" + hub.origin();
            // This installation's native default record and Keystore remain untouched.
            runOnMainSync(initial::reload);
            ready(initial);
            awaitJs(initial, "document.getElementById('settings').hidden&&document.getElementById('status-toggle').dataset.state==='online'&&!document.getElementById('refresh').disabled", "actual packaged transport did not connect to loopback Hub");
            js(initial, "document.querySelector('#navigation [data-page=activity]').click();document.querySelector('[data-period=month]').click();true");
            awaitJs(initial, "[...document.querySelectorAll('#dashboard .dash-tab')].some(b=>b.textContent==='每日')", "activity subview controls did not render");
            js(initial, "[...document.querySelectorAll('#dashboard .dash-tab')].find(b=>b.textContent==='每日').click();true");
            awaitJs(initial, "document.querySelector('#dashboard .dash-tab[aria-pressed=true]')?.textContent==='每日'&&!document.getElementById('page-next').disabled", "daily activity fixture did not produce multiple pages");
            js(initial, "document.getElementById('page-next').click();true");
            String position = js(initial, "document.getElementById('page-position').textContent");
            require(!position.startsWith("\"1 /"), "actual pager failed to advance");
            awaitJs(initial, "(()=>{const s=JSON.parse(localStorage.getItem(" + q(readingKey) + "));return s?.page==='activity'&&s.period==='month'&&s.activityKind==='day'&&s.index>0})()", "actual dashboard did not save reading position");
            MainActivity owner = activity;
            terminate(initial);
            await(() -> webView() != null && webView() != initial, "loopback renderer did not recover");
            WebView replacement = webView();
            ready(replacement);
            awaitJs(replacement, "document.querySelector('#navigation [aria-current=page]')?.dataset.page==='activity'&&document.querySelector('[data-period=month]').getAttribute('aria-pressed')==='true'&&document.querySelector('#dashboard .dash-tab[aria-pressed=true]')?.textContent==='每日'", "visible dashboard reading state did not restore after renderer loss");
            awaitJs(replacement, "document.getElementById('page-position').textContent===" + position, "visible pager position did not restore after renderer loss");
            awaitJs(replacement, "document.getElementById('status-toggle').dataset.state==='online'&&!document.getElementById('refresh').disabled", "recovered dashboard did not finish real loopback reads");
            require(activity == owner && !onMain(owner::isDestroyed), "reading recovery replaced Activity");
            await(() -> hub.stats.get() > 0 && hub.history.get() > 0, "fixture did not receive both fixed Hub GET routes");
            hub.assertRequests();
            // Import the actual packaged transport. Its request ID traverses the real
            // WebView request header, MainActivity proxy and HubHttpClient registry.
            js(replacement, "window.__engineeringTransport=null;" + importing("transport.mjs", "window.__engineeringTransport=m.createTransport({bridge:TokenMonitorAndroid});"));
            awaitModule(replacement, "window.__engineeringTransport!==null", "packaged transport import failed");
            hub.drip = true;
            js(replacement, "window.__engineeringRead='pending';window.__engineeringPublications=0;window.__engineeringTransport.getJSON('/api/stats').then(()=>{window.__engineeringPublications++;window.__engineeringRead='success'},e=>{window.__engineeringRead=e.name});true");
            require(hub.slowStarted.await(5, TimeUnit.SECONDS), "real proxy did not reach slow-drip fixture");
            long cancellation = SystemClock.elapsedRealtime();
            js(replacement, "window.__engineeringTransport.cancel();true");
            while (SystemClock.elapsedRealtime() - cancellation < 1000 && !"\"AbortError\"".equals(js(replacement, "window.__engineeringRead"))) SystemClock.sleep(10);
            require("\"AbortError\"".equals(js(replacement, "window.__engineeringRead")), "packaged cancellation did not reject promptly");
            while (SystemClock.elapsedRealtime() - cancellation < 1000 && activeNativeReads() != 0) SystemClock.sleep(10);
            require(activeNativeReads() == 0 && SystemClock.elapsedRealtime() - cancellation < 1000, "native proxy cancellation exceeded one second");
            hub.releaseSlow.countDown();
            SystemClock.sleep(400);
            require("0".equals(js(replacement, "window.__engineeringPublications")), "cancelled transport published a late response");
            require(position.equals(js(replacement, "document.getElementById('page-position').textContent")), "cancelled response changed visible reading position");
            hub.assertRequests();
            js(replacement, "localStorage.removeItem(" + q(readingKey) + ");true");
        }
        require("unconfigured".equals(new ConnectionStore(getTargetContext()).storageState()), "HTTP fixture changed the native default connection");
    }

    private int activeNativeReads() throws Exception {
        Object client = onMain(() -> field(activity, "dataRequests"));
        synchronized (client) { return ((Map<?, ?>) field(client, "active")).size(); }
    }

    private void recovery() throws Exception {
        launch();
        try (LoopbackHub hub = new LoopbackHub()) {
            AtomicReference<ConnectionStore.Config> recovered = new AtomicReference<>();
            AtomicInteger writes = new AtomicInteger();
            ConnectionStore memory = new ConnectionStore(new ConnectionStore.Storage() {
                @Override public ConnectionStore.StoredRecord read() throws Exception {
                    ConnectionStore.Config value = recovered.get();
                    if (value == null) throw new java.io.IOException("engineering-synthetic-unreadable-record");
                    return new ConnectionStore.StoredRecord(value, false);
                }
                @Override public void write(ConnectionStore.Config config) { throw new AssertionError("recovery fixture requires explicit recovery write"); }
                @Override public void recover(ConnectionStore.Config config, boolean replaceKey) {
                    require(!replaceKey, "read-failed recovery unexpectedly replaced encryption key");
                    recovered.set(config); writes.incrementAndGet();
                }
            });
            require("read_failed".equals(memory.storageState()), "synthetic unreadable store did not enter read-failed state");
            onMain(() -> { Field slot = MainActivity.class.getDeclaredField("connections"); slot.setAccessible(true); slot.set(activity, memory); return null; });
            WebView view = webView();
            runOnMainSync(view::reload);
            ready(view);
            awaitJs(view, "JSON.parse(TokenMonitorAndroid.getConnection()).storageState==='read_failed'&&!document.getElementById('settings').hidden&&!document.getElementById('hub-reload').hidden&&document.getElementById('hub-summary-state').textContent==='连接记录读取失败'", "unreadable native record did not expose recovery UI and retry-read control");
            String payload = new JSONObject().put("source", "hub").put("hubUrl", hub.origin())
                    .put("hubSecret", "engineering-synthetic-secret").put("recover", true).toString();
            require("true".equals(js(view, "(()=>{const r=JSON.parse(TokenMonitorAndroid.saveConnection(" + q(payload) + "));return r.ok===false&&r.error==='connection_recovery_required'})()")), "native recovery accepted a save without a successful probe");
            require(writes.get() == 0 && recovered.get() == null && "read_failed".equals(memory.storageState()), "rejected recovery mutated synthetic storage");
            String requestId = "engineering-recovery-" + UUID.randomUUID();
            js(view, "window.__engineeringRecoveryProbe=null;window.__engineeringRecoveryListener=e=>{if(e.detail?.id!==" + q(requestId) + ")return;window.__engineeringRecoveryProbe=e.detail.ok===true&&e.detail.source==='hub'&&e.detail.origin===" + q(hub.origin()) + ";window.removeEventListener('tokenmonitor:connection-test',window.__engineeringRecoveryListener)};window.addEventListener('tokenmonitor:connection-test',window.__engineeringRecoveryListener);TokenMonitorAndroid.testConnection(" + q(payload) + "," + q(requestId) + ");true");
            awaitJs(view, "window.__engineeringRecoveryProbe===true", "real native recovery probe did not report successful matching connection");
            require(hub.stats.get() == 1 && writes.get() == 0, "recovery probe did not use one fixed GET without saving");
            hub.assertRequests();
            require("true".equals(js(view, "JSON.parse(TokenMonitorAndroid.saveConnection(" + q(payload) + ")).ok===true")), "matching recovery save failed after successful native probe");
            require(writes.get() == 1 && recovered.get() != null && hub.origin().equals(recovered.get().hubUrl)
                    && "engineering-synthetic-secret".equals(recovered.get().hubSecret) && "ready".equals(memory.storageState()), "authorized recovery did not commit exactly one synthetic record");
            require("true".equals(js(view, "(()=>{const r=JSON.parse(TokenMonitorAndroid.getConnection());return r.storageState==='ready'&&r.hasHubSecret===true&&!('hubSecret' in r)})()")), "recovered bridge projection is unavailable or exposes a secret");
        }
        require("unconfigured".equals(new ConnectionStore(getTargetContext()).storageState()), "recovery fixture changed native default connection");
    }

    /** Bound exclusively to this device's IPv4 loopback; all workers/sockets close with the check. */
    private static final class LoopbackHub implements AutoCloseable {
        final ServerSocket server;
        final ExecutorService workers = Executors.newCachedThreadPool(task -> { Thread thread = new Thread(task, "engineering-loopback"); thread.setDaemon(true); return thread; });
        final Set<Socket> sockets = ConcurrentHashMap.newKeySet();
        final AtomicInteger stats = new AtomicInteger(), history = new AtomicInteger(), invalid = new AtomicInteger();
        final CountDownLatch slowStarted = new CountDownLatch(1), releaseSlow = new CountDownLatch(1);
        final byte[] statsBody, historyBody;
        volatile boolean drip, closed;
        LoopbackHub() throws Exception {
            JSONObject period = new JSONObject().put("totalTokens", 1234).put("costUsd", 1).put("sessions", new JSONObject()).put("projects", new JSONObject());
            statsBody = new JSONObject().put("devices", new JSONArray()).put("updatedAt", "2026-10-26T00:00:00Z")
                    .put("limits", new JSONObject().put("providers", new JSONArray()))
                    .put("periods", new JSONObject().put("today", period).put("month", period).put("allTime", period)).toString().getBytes(StandardCharsets.UTF_8);
            JSONArray days = new JSONArray();
            for (int i = 0; i < 120; i++) days.put(new JSONObject().put("date", java.time.LocalDate.of(2026, 10, 25).minusDays(119 - i).toString()).put("tokens", (i + 1) * 100).put("cost", 1));
            historyBody = new JSONObject().put("daily", days).put("monthly", new JSONArray().put(new JSONObject().put("month", "2026-10").put("tokens", 32500)))
                    .put("summary", new JSONObject().put("totalTokens", 32500).put("activeDays", 25)).toString().getBytes(StandardCharsets.UTF_8);
            server = new ServerSocket(0, 8, InetAddress.getByName("127.0.0.1"));
            workers.execute(() -> {
                while (!closed) {
                    try { Socket socket = server.accept(); sockets.add(socket); workers.execute(() -> serve(socket)); }
                    catch (Exception failure) { if (!closed) invalid.incrementAndGet(); }
                }
            });
        }
        String origin() { return "http://127.0.0.1:" + server.getLocalPort(); }
        void assertRequests() { require(invalid.get() == 0, "loopback received invalid method, route or synthetic authorization"); }
        private void serve(Socket socket) {
            try (Socket connection = socket) {
                connection.setSoTimeout(5000);
                BufferedReader input = new BufferedReader(new InputStreamReader(connection.getInputStream(), StandardCharsets.US_ASCII));
                String line = input.readLine();
                String[] request = line == null ? new String[0] : line.split(" ");
                boolean statsRoute = request.length == 3 && "GET".equals(request[0]) && "/api/stats".equals(request[1]);
                boolean historyRoute = request.length == 3 && "GET".equals(request[0]) && "/api/history".equals(request[1]);
                boolean authorized = false;
                while ((line = input.readLine()) != null && !line.isEmpty()) {
                    int colon = line.indexOf(':');
                    if (colon > 0 && "authorization".equalsIgnoreCase(line.substring(0, colon))) authorized = "Bearer engineering-synthetic-secret".equals(line.substring(colon + 1).trim());
                }
                if ((!statsRoute && !historyRoute) || !authorized) { invalid.incrementAndGet(); return; }
                if (statsRoute) stats.incrementAndGet(); else history.incrementAndGet();
                byte[] body = statsRoute ? statsBody : historyBody;
                OutputStream output = connection.getOutputStream();
                output.write(("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + body.length + "\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
                if (statsRoute && drip) {
                    output.write(body, 0, 1); output.flush(); slowStarted.countDown();
                    releaseSlow.await(8, TimeUnit.SECONDS);
                    output.write(body, 1, body.length - 1);
                } else output.write(body);
                output.flush();
            } catch (Exception ignored) { /* Peer abort is the expected cancellation outcome. */ }
            finally { sockets.remove(socket); }
        }
        @Override public void close() throws Exception {
            closed = true; releaseSlow.countDown(); server.close();
            for (Socket socket : sockets) try { socket.close(); } catch (Exception ignored) { }
            workers.shutdownNow();
            require(workers.awaitTermination(3, TimeUnit.SECONDS), "loopback fixture workers did not stop");
        }
    }

    private void terminate(WebView view) throws Exception {
        require(onMain(() -> {
            WebViewRenderProcess process = view.getWebViewRenderProcess();
            return process != null && process.terminate();
        }), "WebView renderer termination unavailable or rejected");
    }

    private void ready(WebView view) throws Exception {
        require(view != null, "Activity has no WebView");
        awaitJs(view, "document.readyState==='complete'&&typeof window.tokenMonitorBack==='function'&&document.getElementById('connection')?.textContent!=='读取中'", "packaged UI did not initialize");
        require(!onMain(activity::isDestroyed), "Activity destroyed during UI initialization");
    }

    private void assertViewport(WebView view, boolean editor) throws Exception {
        awaitJs(view, "(()=>{const d=document.documentElement;return innerHeight>0&&innerWidth>0&&scrollX===0&&scrollY===0&&d.scrollWidth<=innerWidth+2&&d.scrollHeight<=innerHeight+2})()", "document overflowed fixed viewport");
        if (editor) awaitJs(view, "(()=>{const ids=['connection-url','connection-save','connection-cancel'];return ids.every(id=>{const r=document.getElementById(id).getBoundingClientRect();return r.width>0&&r.height>0&&r.top>=-2&&r.bottom<=innerHeight+2&&r.left>=-2&&r.right<=innerWidth+2})})()", "keyboard obscured input or editor actions");
    }

    private WebView webView() throws Exception { return onMain(() -> (WebView) field(activity, "webView")); }
    private Rect frame() throws Exception {
        return onMain(() -> { Rect result = new Rect(); activity.getWindow().getDecorView().getWindowVisibleDisplayFrame(result); return result; });
    }
    private void closeActivity() throws Exception {
        if (activity == null) return;
        MainActivity closing = activity;
        runOnMainSync(() -> { if (!closing.isDestroyed()) closing.finish(); });
        await(() -> onMain(closing::isDestroyed), "debug Activity did not finish during cleanup");
        requireStopped(closing);
        activity = null;
    }
    private void requireStopped(MainActivity owner) throws Exception {
        require(onMain(() -> {
            DeviceStatusController controller = (DeviceStatusController) field(owner, "deviceStatus");
            return controller != null && !((Boolean) field(controller, "active"))
                    && !((Boolean) field(controller, "receiverRegistered")) && field(controller, "networkCallback") == null;
        }), "destroyed Activity retained device-status subscriptions");
    }
    private String js(WebView view, String expression) throws Exception {
        CountDownLatch done = new CountDownLatch(1);
        AtomicReference<String> value = new AtomicReference<>();
        runOnMainSync(() -> view.evaluateJavascript(expression, result -> { value.set(result); done.countDown(); }));
        require(done.await(5, TimeUnit.SECONDS), "WebView JavaScript callback timed out");
        return value.get();
    }
    private void awaitJs(WebView view, String expression, String message) throws Exception {
        await(() -> "true".equals(js(view, expression)), message);
    }
    private static String importing(String path, String success) {
        // evaluateJavascript has no module referrer on some WebView releases.
        // Always use the full packaged origin instead of resolving against eval.
        return "window.__engineeringModuleError=null;import(" + q(NativePolicy.LOCAL_ORIGIN + "/" + path)
                + ").then(m=>{" + success + "}).catch(e=>{window.__engineeringModuleError="
                + "['TypeError','SyntaxError','ReferenceError','SecurityError','NotAllowedError','Error'].includes(e&&e.name)?e.name:'module_error'});true";
    }
    private void awaitModule(WebView view, String expression, String message) throws Exception {
        await(() -> {
            String category = js(view, "window.__engineeringModuleError||null");
            require("null".equals(category), message + " (module category " + category + ")");
            return "true".equals(js(view, expression));
        }, message);
    }
    private void await(Condition condition, String message) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + TIMEOUT_MS;
        while (SystemClock.elapsedRealtime() < deadline) {
            if (condition.get()) return;
            SystemClock.sleep(80);
        }
        throw new AssertionError(message);
    }
    private <T> T onMain(Value<T> action) throws Exception {
        // Insets checks may call another helper already executing on the UI thread.
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) return action.get();
        AtomicReference<T> result = new AtomicReference<>();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        runOnMainSync(() -> { try { result.set(action.get()); } catch (Throwable error) { failure.set(error); } });
        if (failure.get() instanceof Exception) throw (Exception) failure.get();
        if (failure.get() instanceof Error) throw (Error) failure.get();
        return result.get();
    }
    private static Object field(Object owner, String name) throws Exception {
        Field field = owner.getClass().getDeclaredField(name); field.setAccessible(true); return field.get(owner);
    }
    private static String q(String value) { return JSONObject.quote(value); }
    private static String restore(String key, Object value) {
        return value == null || value == JSONObject.NULL ? "localStorage.removeItem(" + q(key) + ");"
                : "localStorage.setItem(" + q(key) + "," + q(value.toString()) + ");";
    }
    private static String safeFailure(Throwable error) {
        // Assertions are authored here. Framework exception messages are deliberately
        // omitted because OEM/WebView diagnostics may include private connection data.
        StringBuilder value = new StringBuilder(error.getClass().getName());
        if (error instanceof AssertionError) value.append(": ").append(error.getMessage());
        for (StackTraceElement frame : error.getStackTrace()) {
            value.append("\n  at ").append(frame);
            if (value.length() > 2400) break;
        }
        return value.toString();
    }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError(message); }
    private interface Task { void run() throws Exception; }
    private interface Condition { boolean get() throws Exception; }
    private interface Value<T> { T get() throws Exception; }
    private static final class Check {
        final String name; final Task body;
        Check(String name, Task body) { this.name = name; this.body = body; }
    }
}
