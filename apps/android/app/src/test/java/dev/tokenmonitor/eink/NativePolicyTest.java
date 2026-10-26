package dev.tokenmonitor.eink;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashSet;
import java.util.Arrays;
import java.util.Set;
import java.util.stream.Stream;

/** Security boundaries exercised without an emulator or Android framework mocks. */
public final class NativePolicyTest {
    public static void main(String[] args) throws Exception {
        if (args.length != 1) throw new AssertionError("Pass the staged UI assets directory");
        Path assetsRoot = Paths.get(args[0]);
        Set<String> bundled = new HashSet<>();
        try (Stream<Path> paths = Files.walk(assetsRoot)) {
            paths.filter(Files::isRegularFile).forEach(path ->
                    bundled.add("/" + assetsRoot.relativize(path).toString().replace('\\', '/')));
        }
        if (!bundled.equals(NativePolicy.ASSETS))
            throw new AssertionError("Bundled UI assets differ from native allowlist: " + bundled + " vs " + NativePolicy.ASSETS);
        System.out.println("Bundled UI asset allowlist matches " + bundled.size() + " files");
        if (!NativePolicy.ROUTES.equals(new HashSet<>(Arrays.asList("/api/stats", "/api/history"))))
            throw new AssertionError("Only Hub stats and history routes are allowed");
        equal("http://192.168.1.7:18787", NativePolicy.normalizeOrigin("http://192.168.1.7:18787/"));
        equal("https://example.org", NativePolicy.normalizeOrigin("HTTPS://EXAMPLE.ORG:443"));
        equal("http://[::1]:18787", NativePolicy.normalizeOrigin("http://[::1]:18787"));
        for (String invalid : new String[]{"ftp://example.org", "http://user:secret@example.org", "http://example.org/api",
                "http://example.org?x=1", "http://example.org#secret", "http://example.org:", "http://example.org:0",
                "http://example.org:65536", "http://example.org:-2", "http://example.org\n", " http://example.org",
                "http://example.org/%2e%2e", "http://example.org:hello", "http://example.org\\@evil.org"}) {
            try { NativePolicy.normalizeOrigin(invalid); throw new AssertionError("Accepted invalid origin"); }
            catch (IllegalArgumentException expected) { }
        }
        if (!NativePolicy.isLocalRequest("https://appassets.androidplatform.net/api/stats")) throw new AssertionError();
        for (String invalid : new String[]{"https://appassets.androidplatform.net.evil.org/api/stats", "http://appassets.androidplatform.net/api/stats",
                "https://appassets.androidplatform.net:443/api/stats", "https://appassets.androidplatform.net/api/stats?remote=1", "file:///index.html"}) {
            if (NativePolicy.isLocalRequest(invalid)) throw new AssertionError("Accepted external request");
        }
        try { NativePolicy.checkSecret("token\r\nCookie: leak"); throw new AssertionError(); }
        catch (IllegalArgumentException expected) { }
        equal("retained", NativePolicy.selectSecret(null, "http://host-a", "http://host-a", "retained"));
        equal("", NativePolicy.selectSecret(null, "http://host-b", "http://host-a", "must-not-leak"));
        equal("", NativePolicy.selectSecret("", "http://host-a", "http://host-a", "cleared"));
        equal("new", NativePolicy.selectSecret("new", "http://host-b", "http://host-a", "old"));
        equal("", NativePolicy.selectSecret(null, "", "http://host-a", "cleared"));
        byte[] exact = new byte[NativePolicy.MAX_RESPONSE_BYTES];
        if (NativePolicy.readLimited(new ByteArrayInputStream(exact)).length != exact.length) throw new AssertionError();
        try { NativePolicy.readLimited(new ByteArrayInputStream(new byte[exact.length + 1])); throw new AssertionError(); }
        catch (IOException expected) { }
        System.out.println("Native policy checks passed");
    }
    private static void equal(String expected, String actual) {
        if (!expected.equals(actual)) throw new AssertionError(expected + " != " + actual);
    }
}
