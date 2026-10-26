package dev.tokenmonitor.eink;

import java.lang.reflect.Field;
import java.util.concurrent.atomic.AtomicLong;

public final class RecoveryPermitTest {
    public static void main(String[] args) throws Exception {
        normalizedOriginAndOneUse();
        expiresAndFailsClosedOnClockChange();
        everyCandidateMismatchConsumes();
        clearAndNoPlaintextFields();
        System.out.println("Recovery permit checks passed");
    }

    private static void normalizedOriginAndOneUse() {
        AtomicLong clock = new AtomicLong(10);
        RecoveryPermit permit = new RecoveryPermit(clock::get);
        permit.issue("HTTPS://Hub.Example:443/", "test-secret", "storage-a", 4, 8);
        check(permit.consume("https://hub.example", "test-secret", "storage-a", 4, 8),
                "normalized origin should match");
        check(!permit.consume("https://hub.example", "test-secret", "storage-a", 4, 8),
                "permit is one use");
    }

    private static void expiresAndFailsClosedOnClockChange() {
        AtomicLong clock = new AtomicLong(100);
        RecoveryPermit permit = new RecoveryPermit(clock::get);
        permit.issue("https://hub.example", "secret", "state", 1, 2);
        clock.set(30_099);
        check(permit.consume("https://hub.example", "secret", "state", 1, 2), "permit valid before 30 seconds");

        permit.issue("https://hub.example", "secret", "state", 1, 2);
        clock.addAndGet(30_000);
        check(!permit.consume("https://hub.example", "secret", "state", 1, 2), "permit expires at 30 seconds");

        permit.issue("https://hub.example", "secret", "state", 1, 2);
        clock.decrementAndGet();
        check(!permit.consume("https://hub.example", "secret", "state", 1, 2), "backwards clock fails closed");
    }

    private static void everyCandidateMismatchConsumes() {
        mismatchConsumes((permit) -> permit.consume("https://other.example", "secret", "state", 1, 2), "origin");
        mismatchConsumes((permit) -> permit.consume("https://hub.example", "other-secret", "state", 1, 2), "secret");
        mismatchConsumes((permit) -> permit.consume("https://hub.example", "secret", "other-state", 1, 2), "storage state");
        mismatchConsumes((permit) -> permit.consume("https://hub.example", "secret", "state", 9, 2), "revision");
        mismatchConsumes((permit) -> permit.consume("https://hub.example", "secret", "state", 1, 9), "epoch");
    }

    private static void mismatchConsumes(Attempt attempt, String field) {
        AtomicLong clock = new AtomicLong(1000);
        RecoveryPermit permit = new RecoveryPermit(clock::get);
        permit.issue("https://hub.example", "secret", "state", 1, 2);
        check(!attempt.run(permit), field + " mismatch rejected");
        check(!permit.consume("https://hub.example", "secret", "state", 1, 2),
                field + " mismatch consumes permit");
    }

    private static void clearAndNoPlaintextFields() throws Exception {
        AtomicLong clock = new AtomicLong(0);
        RecoveryPermit permit = new RecoveryPermit(clock::get);
        permit.issue("https://hub.example", "do-not-store-this-secret", "state", 1, 2);
        permit.clear();
        check(!permit.consume("https://hub.example", "do-not-store-this-secret", "state", 1, 2), "clear revokes permit");
        for (Field field : RecoveryPermit.class.getDeclaredFields()) {
            String name = field.getName().toLowerCase(java.util.Locale.ROOT);
            check(!name.contains("origin") && !name.contains("secret"), "no origin or secret field: " + field.getName());
            check(field.getType() != String.class || field.getName().equals("storageState"),
                    "only storage state may be retained as text: " + field.getName());
            check(field.getType() != byte[].class || field.getName().equals("fingerprint"),
                    "only fingerprint may be a byte array: " + field.getName());
        }
    }

    private interface Attempt { boolean run(RecoveryPermit permit); }
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
