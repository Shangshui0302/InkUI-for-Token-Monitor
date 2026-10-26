package dev.tokenmonitor.eink;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.URI;
import java.net.URISyntaxException;
import java.util.Locale;
import java.util.Set;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;

final class NativePolicy {
    static final String LOCAL_ORIGIN = "https://appassets.androidplatform.net";
    static final int MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
    static final Set<String> ASSETS = immutableSet("/index.html", "/styles.css", "/themes.css",
            "/dashboard.css", "/app.js", "/model.mjs", "/dashboard-model.mjs",
            "/dashboard.js", "/theme.mjs", "/connection.mjs", "/connection-recovery.mjs", "/transport.mjs", "/charts.mjs", "/quota-presentation.mjs", "/quota-layout.mjs", "/provider-order.mjs", "/gestures.mjs", "/display.mjs", "/icons.mjs", "/device-status.mjs", "/settings.css", "/settings.js", "/settings-connection.mjs");
    static final Set<String> ROUTES = immutableSet("/api/stats", "/api/history");

    private static Set<String> immutableSet(String... values) {
        return Collections.unmodifiableSet(new HashSet<>(Arrays.asList(values)));
    }

    static String normalizeOrigin(String value) {
        if (value == null || value.isEmpty()) return "";
        if (value.length() > 2048 || !value.equals(value.trim())) throw new IllegalArgumentException();
        for (int i = 0; i < value.length(); i++) {
            if (Character.isWhitespace(value.charAt(i)) || Character.isISOControl(value.charAt(i))) throw new IllegalArgumentException();
        }
        try {
            URI uri = new URI(value);
            String scheme = uri.getScheme() == null ? "" : uri.getScheme().toLowerCase(Locale.ROOT);
            if ((!scheme.equals("http") && !scheme.equals("https")) || uri.getHost() == null
                    || uri.getUserInfo() != null || uri.getRawQuery() != null || uri.getRawFragment() != null
                    || !(uri.getRawPath().isEmpty() || uri.getRawPath().equals("/"))
                    || uri.getPort() == 0 || uri.getPort() > 65535 || uri.getPort() < -1) throw new IllegalArgumentException();
            // URI accepts a bare trailing colon as an unspecified port. Reject it explicitly.
            if (uri.getRawAuthority().endsWith(":")) throw new IllegalArgumentException();
            String host = uri.getHost().toLowerCase(Locale.ROOT);
            int port = uri.getPort();
            if ((scheme.equals("http") && port == 80) || (scheme.equals("https") && port == 443)) port = -1;
            return scheme + "://" + host + (port == -1 ? "" : ":" + port);
        } catch (URISyntaxException e) { throw new IllegalArgumentException(); }
    }

    static String checkSecret(String secret) {
        if (secret == null || secret.length() > 8192) throw new IllegalArgumentException();
        for (int i = 0; i < secret.length(); i++) {
            char c = secret.charAt(i);
            if (c < 0x20 || c > 0x7e) throw new IllegalArgumentException();
        }
        return secret;
    }

    static String selectSecret(String candidate, String origin, String previousOrigin, String previousSecret) {
        if (candidate != null) checkSecret(candidate);
        if (origin.isEmpty()) return "";
        return candidate != null ? candidate : origin.equals(previousOrigin) ? previousSecret : "";
    }

    static boolean isLocalRequest(String rawUrl) {
        try {
            URI uri = new URI(rawUrl);
            return "https".equals(uri.getScheme()) && "appassets.androidplatform.net".equals(uri.getHost())
                    && uri.getPort() == -1 && uri.getUserInfo() == null && uri.getRawQuery() == null
                    && uri.getRawFragment() == null;
        } catch (URISyntaxException e) { return false; }
    }

    static byte[] readLimited(InputStream stream) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        byte[] buffer = new byte[16384];
        int count;
        while ((count = stream.read(buffer)) != -1) {
            if (count > MAX_RESPONSE_BYTES - output.size()) throw new IOException("response_limit");
            output.write(buffer, 0, count);
        }
        return output.toByteArray();
    }
}
