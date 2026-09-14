package com.codexmonitor.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Context;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.WindowManager;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.InputMethodManager;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.ByteArrayInputStream;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.util.Collections;

public final class MainActivity extends Activity {
    private static final String PREFS = "node-settings";
    private static final String PREF_URL = "node-url";
    private static final String PREF_HIDE_STATUS = "hide-status-bar";
    private static final String PREF_HIDE_NAVIGATION = "hide-navigation-bar";
    private static final long MAIN_DOCUMENT_TIMEOUT_MS = 30000L;

    private WebView webView;
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private Runnable mainDocumentTimeout;
    private String activeMainFrameUrl;
    private long mainNavigationId;
    private boolean mainNavigationPending;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String savedUrl = getPreferences(MODE_PRIVATE).getString(PREF_URL, "");
        if (savedUrl.isEmpty()) {
            showSetup(null);
        } else {
            showWebView(savedUrl);
        }
    }

    @Override
    public void onBackPressed() {
        if (webView != null && webView.canGoBack()) {
            webView.goBack();
        } else if (webView != null) {
            showRootMenu();
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void onDestroy() {
        destroyWebView();
        super.onDestroy();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus && webView != null) {
            applySystemBars();
        }
    }

    private void showSetup(String currentUrl) {
        showSetup(currentUrl, null, false);
    }

    private void showSetup(String currentUrl, String error, boolean recovery) {
        showSystemBars();
        destroyWebView();

        int padding = dp(20);
        LinearLayout panel = new LinearLayout(this);
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setPadding(padding, padding, padding, padding);
        panel.setBackgroundColor(Color.WHITE);

        ScrollView scrollView = new ScrollView(this);
        scrollView.setFillViewport(true);
        scrollView.addView(panel, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView title = new TextView(this);
        title.setText("Codex Monitor");
        title.setTextSize(22);
        title.setTextColor(Color.rgb(28, 35, 42));
        panel.addView(title, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView help = new TextView(this);
        help.setText("Enter the web address of a Codex Monitor node. The address is stored only on this device.");
        help.setTextSize(15);
        help.setTextColor(Color.rgb(75, 84, 93));
        LinearLayout.LayoutParams helpParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        helpParams.topMargin = dp(12);
        panel.addView(help, helpParams);

        if (error != null && !error.isEmpty()) {
            TextView errorText = new TextView(this);
            errorText.setText(error);
            errorText.setTextSize(15);
            errorText.setTextColor(Color.rgb(165, 45, 45));
            LinearLayout.LayoutParams errorParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            errorParams.topMargin = dp(16);
            panel.addView(errorText, errorParams);
        }

        EditText input = new EditText(this);
        input.setSingleLine(true);
        input.setHint("https://… or http://192.168.x.x:port");
        input.setInputType(android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_VARIATION_URI);
        input.setText(currentUrl == null ? "" : currentUrl);
        LinearLayout.LayoutParams inputParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        inputParams.topMargin = dp(18);
        panel.addView(input, inputParams);

        Button connect = new Button(this);
        connect.setText("Save and connect");
        LinearLayout.LayoutParams buttonParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        buttonParams.topMargin = dp(12);
        panel.addView(connect, buttonParams);

        connect.setOnClickListener(view -> connectToNode(input));

        if (recovery) {
            Button retry = new Button(this);
            retry.setText("Retry this address");
            LinearLayout.LayoutParams retryParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            retryParams.topMargin = dp(8);
            panel.addView(retry, retryParams);
            retry.setOnClickListener(view -> connectToNode(input));

            Button exit = new Button(this);
            exit.setText("Exit");
            LinearLayout.LayoutParams exitParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            exitParams.topMargin = dp(4);
            panel.addView(exit, exitParams);
            exit.setOnClickListener(view -> finish());
        }

        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        setContentView(scrollView);
    }

    private void connectToNode(EditText input) {
        String url = input.getText().toString().trim();
        if (!isAllowedNodeUrl(url)) {
            input.setError("Use an http(s) URL; HTTP must point to a local address.");
            return;
        }
        getPreferences(MODE_PRIVATE).edit().putString(PREF_URL, url).apply();
        ((InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE))
                .hideSoftInputFromWindow(input.getWindowToken(), 0);
        showWebView(url);
    }

    private void showRootMenu() {
        final String currentUrl = activeMainFrameUrl != null
                ? activeMainFrameUrl
                : getPreferences(MODE_PRIVATE).getString(PREF_URL, "");
        new AlertDialog.Builder(this)
                .setTitle("Codex Monitor")
                .setItems(new String[]{"Change address", "Retry", "Exit"}, (dialog, which) -> {
                    if (which == 0) {
                        showSetup(currentUrl);
                    } else if (which == 1) {
                        if (webView != null) {
                            scheduleMainDocumentTimeout(webView, currentUrl);
                            webView.reload();
                        } else if (!currentUrl.isEmpty()) {
                            showWebView(currentUrl);
                        }
                    } else if (which == 2) {
                        finish();
                    }
                })
                .setNegativeButton("Cancel", (dialog, which) -> dialog.dismiss())
                .show();
    }

    private void destroyWebView() {
        WebView oldWebView = webView;
        webView = null;
        activeMainFrameUrl = null;
        mainNavigationId++;
        clearMainDocumentTimeout();
        if (oldWebView != null) {
            oldWebView.stopLoading();
            oldWebView.destroy();
        }
    }

    private void connectFailure(WebView view, String url, String reason) {
        if (view != webView || url == null || !isActiveMainFrameUrl(url)) {
            return;
        }
        showSetup(url, reason, true);
    }

    private boolean isActiveMainFrameUrl(String url) {
        if (activeMainFrameUrl == null) {
            return true;
        }
        return activeMainFrameUrl.equals(url);
    }

    private void scheduleMainDocumentTimeout(WebView view, String url) {
        clearMainDocumentTimeout();
        startMainDocumentTimeout(view, url);
    }

    private void startMainDocumentTimeout(WebView view, String url) {
        if (mainNavigationPending) {
            activeMainFrameUrl = url;
            return;
        }
        final long navigationId = ++mainNavigationId;
        mainNavigationPending = true;
        activeMainFrameUrl = url;
        mainDocumentTimeout = () -> {
            String failedUrl = activeMainFrameUrl;
            if (view == webView && navigationId == mainNavigationId
                    && failedUrl != null) {
                connectFailure(view, failedUrl,
                        "The page did not finish loading within 30 seconds. Check the address and network, then retry or edit it.");
            }
        };
        mainHandler.postDelayed(mainDocumentTimeout, MAIN_DOCUMENT_TIMEOUT_MS);
    }

    private void clearMainDocumentTimeout() {
        if (mainDocumentTimeout != null) {
            mainHandler.removeCallbacks(mainDocumentTimeout);
            mainDocumentTimeout = null;
        }
        mainNavigationPending = false;
    }

    private void showWebView(String url) {
        destroyWebView();
        webView = new WebView(this);
        final WebView thisWebView = webView;
        webView.setBackgroundColor(Color.WHITE);
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setUserAgentString(settings.getUserAgentString() + " CodexMonitorAndroid/0.1.0");
        webView.addJavascriptInterface(new AndroidChromeBridge(thisWebView), "CodexMonitorAndroid");
        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri requestedUri = request.getUrl();
                if (view == webView && request.isForMainFrame() && mayLoad(requestedUri)) {
                    scheduleMainDocumentTimeout(view, requestedUri.toString());
                }
                return !mayLoad(requestedUri);
            }

            @Override
            public boolean shouldOverrideUrlLoading(WebView view, String requestedUrl) {
                if (view == webView && mayLoad(Uri.parse(requestedUrl))) {
                    scheduleMainDocumentTimeout(view, requestedUrl);
                }
                return !mayLoad(Uri.parse(requestedUrl));
            }

            @Override
            public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                if (!mayLoad(request.getUrl())) {
                    return blockedResponse();
                }
                return super.shouldInterceptRequest(view, request);
            }

            @Override
            public void onPageStarted(WebView view, String url, android.graphics.Bitmap favicon) {
                super.onPageStarted(view, url, favicon);
                if (view == webView && (view.getUrl() == null || url.equals(view.getUrl()))) {
                    if (mainNavigationPending) {
                        activeMainFrameUrl = url;
                    } else {
                        startMainDocumentTimeout(view, url);
                    }
                }
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                super.onPageFinished(view, url);
                if (view == webView && isActiveMainFrameUrl(url)
                        && (view.getUrl() == null || url.equals(view.getUrl()))) {
                    activeMainFrameUrl = url;
                    clearMainDocumentTimeout();
                }
            }

            @Override
            public void onPageCommitVisible(WebView view, String url) {
                super.onPageCommitVisible(view, url);
                if (view == webView && isActiveMainFrameUrl(url)
                        && (view.getUrl() == null || url.equals(view.getUrl()))) {
                    activeMainFrameUrl = url;
                    clearMainDocumentTimeout();
                }
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest request,
                    android.webkit.WebResourceError error) {
                super.onReceivedError(view, request, error);
                if (request.isForMainFrame()) {
                    connectFailure(view, request.getUrl().toString(),
                            "Could not load the page: " + error.getDescription());
                }
            }

            @Override
            public void onReceivedHttpError(WebView view, WebResourceRequest request,
                    WebResourceResponse response) {
                super.onReceivedHttpError(view, request, response);
                if (request.isForMainFrame() && response.getStatusCode() >= 400) {
                    connectFailure(view, request.getUrl().toString(),
                            "The server returned HTTP " + response.getStatusCode()
                                    + " " + response.getReasonPhrase() + ". Check the address or retry.");
                }
            }
        });
        setContentView(webView);
        applySystemBars();
        startMainDocumentTimeout(webView, url);
        webView.loadUrl(url);
    }

    private void applySystemBars() {
        int flags = View.SYSTEM_UI_FLAG_LAYOUT_STABLE;
        if (getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_STATUS, true)) {
            flags |= View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN;
        }
        if (getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_NAVIGATION, true)) {
            flags |= View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY;
        }
        getWindow().getDecorView().setSystemUiVisibility(flags);
    }

    private void showSystemBars() {
        getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
    }

    private final class AndroidChromeBridge {
        private final WebView owner;

        AndroidChromeBridge(WebView owner) {
            this.owner = owner;
        }

        @JavascriptInterface
        public boolean getHideStatusBar() {
            return getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_STATUS, true);
        }

        @JavascriptInterface
        public boolean getHideNavigationBar() {
            return getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_NAVIGATION, true);
        }

        @JavascriptInterface
        public void setHideStatusBar(boolean hide) {
            getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_HIDE_STATUS, hide).apply();
            runOnUiThread(MainActivity.this::applySystemBars);
        }

        @JavascriptInterface
        public void setHideNavigationBar(boolean hide) {
            getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_HIDE_NAVIGATION, hide).apply();
            runOnUiThread(MainActivity.this::applySystemBars);
        }

        @JavascriptInterface
        public void showNodeAddressSettings() {
            runOnUiThread(() -> {
                if (webView == owner) {
                    showSetup(getPreferences(MODE_PRIVATE).getString(PREF_URL, ""));
                }
            });
        }
    }

    private boolean mayLoad(Uri uri) {
        String scheme = uri.getScheme();
        if ("https".equalsIgnoreCase(scheme)) {
            return true;
        }
        return "http".equalsIgnoreCase(scheme) && isLocalHttpHost(uri.getHost());
    }

    private static WebResourceResponse blockedResponse() {
        return new WebResourceResponse(
                "text/plain", "UTF-8", 403, "Blocked by local HTTP policy",
                Collections.<String, String>emptyMap(),
                new ByteArrayInputStream(new byte[0]));
    }

    private static boolean isAllowedNodeUrl(String value) {
        try {
            Uri uri = Uri.parse(value);
            String scheme = uri.getScheme();
            if (uri.getHost() == null || uri.getUserInfo() != null
                    || !("https".equalsIgnoreCase(scheme) || "http".equalsIgnoreCase(scheme))) {
                return false;
            }
            return "https".equalsIgnoreCase(scheme) || isLocalHttpHost(uri.getHost());
        } catch (RuntimeException ex) {
            return false;
        }
    }

    private static boolean isLocalHttpHost(String host) {
        if (host == null) {
            return false;
        }
        String value = host.toLowerCase(java.util.Locale.ROOT);
        if ("localhost".equals(value) || value.endsWith(".localhost") || value.endsWith(".local")) {
            return true;
        }
        if (value.indexOf(':') >= 0) {
            try {
                InetAddress address = InetAddress.getByName(value);
                return address.isLoopbackAddress() || address.isLinkLocalAddress()
                        || (address instanceof Inet6Address
                        && (address.isSiteLocalAddress() || isUniqueLocalAddress((Inet6Address) address)));
            } catch (Exception ex) {
                return false;
            }
        }
        String[] octets = value.split("\\.");
        if (octets.length != 4) {
            return false;
        }
        int[] ip = new int[4];
        for (int i = 0; i < octets.length; i++) {
            try {
                ip[i] = Integer.parseInt(octets[i]);
            } catch (NumberFormatException ex) {
                return false;
            }
            if (ip[i] < 0 || ip[i] > 255) {
                return false;
            }
        }
        return ip[0] == 10
                || ip[0] == 127
                || ip[0] == 192 && ip[1] == 168
                || ip[0] == 172 && ip[1] >= 16 && ip[1] <= 31
                || ip[0] == 169 && ip[1] == 254;
    }

    private static boolean isUniqueLocalAddress(Inet6Address address) {
        int first = address.getAddress()[0] & 0xff;
        return first == 0xfc || first == 0xfd;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
