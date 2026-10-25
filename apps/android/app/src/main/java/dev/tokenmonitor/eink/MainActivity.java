package dev.tokenmonitor.eink;

import android.app.Activity;
import android.os.Bundle;
import android.view.View;
import android.view.WindowInsets;
import android.webkit.CookieManager;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import org.json.JSONObject;
import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/** The WebView can read packaged UI and three native GET routes only. */
public final class MainActivity extends Activity {
    private WebView webView;
    private ConnectionStore connections;
    private volatile boolean foreground;
    private final Set<HttpURLConnection> activeRequests = ConcurrentHashMap.newKeySet();

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        connections = new ConnectionStore(this);
        webView = new WebView(this);
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG);
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setAllowFileAccessFromFileURLs(false);
        settings.setAllowUniversalAccessFromFileURLs(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setSupportMultipleWindows(false);
        settings.setMediaPlaybackRequiresUserGesture(true);
        settings.setGeolocationEnabled(false);
        settings.setSaveFormData(false);
        settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
        CookieManager.getInstance().setAcceptCookie(false);
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, false);
        webView.setVerticalScrollBarEnabled(false);
        webView.setHorizontalScrollBarEnabled(false);
        webView.setOverScrollMode(View.OVER_SCROLL_NEVER);
        webView.addJavascriptInterface(new ConnectionBridge(), "TokenMonitorAndroid");
        webView.setWebViewClient(new LocalClient());
        // Keep the fixed viewport clear of status/navigation bars and the settings keyboard.
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            webView.setOnApplyWindowInsetsListener((view, insets) -> {
                android.graphics.Insets bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.ime());
                view.setPadding(bars.left, bars.top, bars.right, bars.bottom);
                return insets;
            });
        }
        setContentView(webView);
        if (android.os.Build.VERSION.SDK_INT >= 33) {
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::handleBack);
        }
        webView.loadUrl(NativePolicy.LOCAL_ORIGIN + "/index.html");
    }

    private final class ConnectionBridge {
        @JavascriptInterface public String getConnection() {
            try { return connections.snapshot().publicJson().toString(); }
            catch (Exception ignored) { return "{}"; }
        }
        @JavascriptInterface public String saveConnection(String json) { return connections.save(json); }
    }

    private final class LocalClient extends WebViewClient {
        @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) { return true; }
        @Override public boolean shouldOverrideUrlLoading(WebView view, String url) { return true; }
        @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
            if (!NativePolicy.isLocalRequest(request.getUrl().toString())) return error(403, "request_blocked");
            if (!"GET".equals(request.getMethod())) return error(405, "method_not_allowed");
            String path = request.getUrl().getEncodedPath();
            if (NativePolicy.ROUTES.contains(path)) return proxy(path);
            if (!NativePolicy.ASSETS.contains(path)) return error(404, "not_found");
            try {
                String mime = path.endsWith(".css") ? "text/css" : path.endsWith(".html") ? "text/html" : "application/javascript";
                return response(200, mime, getAssets().open(path.substring(1)));
            } catch (Exception ignored) { return error(503, "ui_unavailable"); }
        }
        @Override public void onPageFinished(WebView view, String url) { dispatchVisibility(); }
    }

    private WebResourceResponse proxy(String path) {
        ConnectionStore.Config config = connections.snapshot();
        if (!foreground) return error(503, "app_background");
        boolean archive = path.equals("/api/session-archive");
        String origin = archive ? config.archiveUrl : config.hubUrl;
        String secret = archive ? config.archiveSecret : config.hubSecret;
        if (origin.isEmpty()) {
            if (!archive) return error(503, "hub_unconfigured");
            return json(200, "{\"available\":false,\"scope\":\"local-device\",\"source\":\"token-monitor-archive\",\"sessions\":{}}");
        }
        HttpURLConnection upstream = null;
        WebResourceResponse result;
        try {
            upstream = (HttpURLConnection) new URL(origin + path).openConnection(Proxy.NO_PROXY);
            upstream.setInstanceFollowRedirects(false);
            upstream.setConnectTimeout(8000);
            upstream.setReadTimeout(8000);
            upstream.setUseCaches(false);
            upstream.setRequestMethod("GET");
            upstream.setRequestProperty("Accept", "application/json");
            upstream.setRequestProperty("Accept-Encoding", "identity");
            if (!secret.isEmpty()) upstream.setRequestProperty("Authorization", "Bearer " + secret);
            activeRequests.add(upstream);
            if (!foreground || !connections.isCurrent(config)) return error(409, "connection_changed");
            int status = upstream.getResponseCode();
            // WebResourceResponse cannot represent redirects; never expose Location or a remote body.
            if (status >= 300 && status <= 399) result = error(502, "upstream_redirect_blocked");
            else if (status == 401 || status == 403) result = error(502, archive ? "archive_unauthorized" : "hub_unauthorized");
            else if (status != 200) result = error(502, archive ? "archive_http_error" : "hub_http_error");
            else if (upstream.getContentLengthLong() > NativePolicy.MAX_RESPONSE_BYTES) result = error(502, "response_too_large");
            else {
                byte[] body;
                try (InputStream in = upstream.getInputStream()) { body = NativePolicy.readLimited(in); }
                JSONObject data = new JSONObject(new String(body, StandardCharsets.UTF_8));
                if (!archive && path.equals("/api/stats") && (data.optJSONArray("devices") == null
                        || data.optJSONObject("limits") == null || data.getJSONObject("limits").optJSONArray("providers") == null)) throw new IllegalArgumentException();
                if (!archive && path.equals("/api/history") && (data.optJSONArray("daily") == null
                        || data.optJSONArray("monthly") == null || data.optJSONObject("summary") == null)) throw new IllegalArgumentException();
                if (archive && (!(data.opt("available") instanceof Boolean)
                        || data.optJSONObject("sessions") == null)) throw new IllegalArgumentException();
                result = json(200, data.toString());
            }
        } catch (org.json.JSONException | IllegalArgumentException ignored) { result = error(502, "invalid_response"); }
        catch (Exception ignored) { result = error(503, archive ? "archive_unavailable" : "hub_unavailable"); }
        finally {
            if (upstream != null) { activeRequests.remove(upstream); upstream.disconnect(); }
        }
        if (!connections.isCurrent(config)) return error(409, "connection_changed");
        if (!foreground) return error(503, "app_background");
        return result;
    }

    private static WebResourceResponse error(int status, String code) {
        return json(status, "{\"error\":\"" + code + "\"}");
    }
    private static WebResourceResponse json(int status, String body) {
        return response(status, "application/json", new ByteArrayInputStream(body.getBytes(StandardCharsets.UTF_8)));
    }
    private static WebResourceResponse response(int status, String mime, InputStream data) {
        Map<String, String> headers = new HashMap<>();
        headers.put("Cache-Control", "no-store"); headers.put("X-Content-Type-Options", "nosniff");
        headers.put("Referrer-Policy", "no-referrer");
        headers.put("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; frame-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'");
        return new WebResourceResponse(mime, "UTF-8", status, status == 200 ? "OK" : "Request failed", headers, data);
    }

    private void dispatchVisibility() {
        if (webView != null) webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:visibility',{detail:{hidden:"
                + (!foreground) + "}}))", null);
    }
    @Override protected void onResume() {
        super.onResume(); foreground = true;
        if (webView != null) { webView.onResume(); dispatchVisibility(); }
    }
    @Override protected void onPause() {
        foreground = false; dispatchVisibility();
        for (HttpURLConnection request : activeRequests) request.disconnect();
        if (webView != null) webView.onPause();
        super.onPause();
    }
    @Override public void onBackPressed() {
        handleBack();
    }
    private void handleBack() {
        webView.evaluateJavascript("Boolean(window.tokenMonitorBack?.())", handled -> {
            if (!"true".equals(handled)) moveTaskToBack(true);
        });
    }
    @Override protected void onDestroy() {
        if (webView != null) {
            webView.removeJavascriptInterface("TokenMonitorAndroid"); webView.stopLoading(); webView.destroy(); webView = null;
        }
        super.onDestroy();
    }
}
