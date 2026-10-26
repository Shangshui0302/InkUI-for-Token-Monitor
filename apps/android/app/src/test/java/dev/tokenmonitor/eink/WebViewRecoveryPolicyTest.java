package dev.tokenmonitor.eink;

import java.util.concurrent.atomic.AtomicLong;

public final class WebViewRecoveryPolicyTest {
    public static void main(String[] args) {
        AtomicLong clock = new AtomicLong(100);
        WebViewRecoveryPolicy policy = new WebViewRecoveryPolicy(clock::get);
        check(policy.allowAutomatic(), "first automatic recovery allowed");
        check(!policy.allowAutomatic(), "repeated failure is throttled");
        clock.addAndGet(59_999);
        check(!policy.allowAutomatic(), "cooldown is rolling and lasts 60 seconds");
        clock.incrementAndGet();
        check(policy.allowAutomatic(), "automatic recovery allowed at 60 seconds");
        check(!policy.allowAutomatic(), "recovery immediately starts another cooldown");

        AtomicLong backwardsClock = new AtomicLong(10_000);
        WebViewRecoveryPolicy backwards = new WebViewRecoveryPolicy(backwardsClock::get);
        check(backwards.allowAutomatic(), "first attempt allowed on arbitrary monotonic origin");
        backwardsClock.set(9_999);
        check(!backwards.allowAutomatic(), "backwards clock fails closed");
        System.out.println("WebView recovery policy checks passed");
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
