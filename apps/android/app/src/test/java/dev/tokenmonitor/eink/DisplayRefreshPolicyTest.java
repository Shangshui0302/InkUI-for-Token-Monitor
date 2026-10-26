package dev.tokenmonitor.eink;

/** Run directly with Java; no Android mocks or panel claims. */
public final class DisplayRefreshPolicyTest {
    public static void main(String[] args) {
        DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
        check(!policy.request("page", 0), "Background changes must be rejected");
        policy.resume(0);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(0));
        for (int page = 1; page <= 6; page++) {
            long now = page * 200L;
            check(policy.request("page", now), "Visible page accepted");
            equal(page == 6 ? DisplayRefreshPolicy.Update.FULL : DisplayRefreshPolicy.Update.PARTIAL, policy.take(now));
        }
        policy.request("page", 1400);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(1400));
        // Data polls do not count as page changes, and a burst has one bounded deadline.
        policy.request("content", 1600);
        policy.request("content", 1650);
        equal(DisplayRefreshPolicy.Update.NONE, policy.take(1719));
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(1720));
        equal(DisplayRefreshPolicy.Update.NONE, policy.take(2000));
        for (int change = 0; change < 6; change++) policy.request("content", 2000 + change);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(2120));
        // Repeated page updates before the compositor fence produce one panel update.
        for (int page = 0; page < 5; page++) policy.request("page", 2400 + page);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(2410));
        policy.request("content", 2600);
        policy.request("full", 2610);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(2610));
        // The minimum interval also applies to manual clear and fast repeated gestures.
        policy.request("full", 2611);
        equal(DisplayRefreshPolicy.Update.NONE, policy.take(2789));
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(2790));
        check(!policy.request("unknown", 3000), "Unknown reasons must be rejected");
        check(!policy.request(null, 3000), "Null reasons must be rejected");
        policy.request("page", 3000);
        policy.pause();
        equal(DisplayRefreshPolicy.Update.NONE, policy.take(5000));
        check(!policy.hasPending(), "Pause drops pending refreshes");
        check(!policy.request("full", 5000), "No refresh while paused");
        policy.resume(6000);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(6000));
        policy.destroy();
        policy.resume(7000);
        check(!policy.isActive(), "Destroy is permanent");
        check(!policy.request("full", 7000), "No refresh after destroy");
        equal(DisplayRefreshPolicy.Update.NONE, policy.take(8000));
        coalescedAndCanceledPages();
        staleVisualFences();
        contentRefreshBudget();
        mixedRefreshBudget();
        fullRefreshResetsBudget();
        OnyxDisplayBackendTest.main(args);
        System.out.println("Display refresh policy checks passed");
    }

    private static void coalescedAndCanceledPages() {
        DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
        policy.resume(0);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(0));
        // Sixty notifications merged into one displayed page spend only one page credit.
        for (int change = 0; change < 60; change++) policy.request("page", 200 + change);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(300));
        for (int page = 2; page <= 6; page++) {
            long now = page * 400L;
            policy.request("page", now);
            policy.request("content", now + 1);
            equal(page == 6 ? DisplayRefreshPolicy.Update.FULL : DisplayRefreshPolicy.Update.PARTIAL,
                    policy.take(now + 2));
        }
        // Pending work dropped by pause cannot consume a page or a partial credit.
        for (int change = 0; change < 60; change++) policy.request("page", 3000 + change);
        policy.pause();
        check(!policy.hasPending(), "Canceled burst clears page intent");
        policy.resume(4000);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(4000));
        policy.request("content", 4400);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(4520));
        for (int page = 1; page <= 6; page++) {
            long now = 4800 + page * 400L;
            policy.request("page", now);
            equal(page == 6 ? DisplayRefreshPolicy.Update.FULL : DisplayRefreshPolicy.Update.PARTIAL,
                    policy.take(now));
        }
    }

    private static void staleVisualFences() {
        DisplayRefreshPolicy.VisualFence fence = new DisplayRefreshPolicy.VisualFence();
        fence.resetRetries();
        long first = fence.begin();
        fence.cancel(); // pause, detach, or a newer DOM update
        check(!fence.complete(first), "Canceled callback cannot consume pending work");
        long second = fence.begin();
        check(!fence.complete(first), "Old callback cannot consume a newer fence");
        check(fence.isWaiting(), "Stale callback leaves the current fence waiting");
        check(fence.complete(second), "Current callback releases its fence");
        check(!fence.complete(second), "Duplicate callback cannot dispatch twice");
        long timedOut = fence.begin();
        check(fence.complete(timedOut), "Watchdog releases a stuck fence");
        fence.cancel();
        check(fence.retry(), "First timeout gets one retry");
        check(!fence.isWaiting(), "Timed out fence does not block the next request");
        check(!fence.complete(timedOut), "Late timed out callback is ignored");
        long retry = fence.begin();
        check(!fence.complete(timedOut), "Late callback cannot consume retry");
        check(fence.complete(retry), "Fresh fence works after timeout");
        check(!fence.retry(), "Second failure cannot create a polling loop");
        check(!fence.retry(), "Synchronous exceptions cannot create a retry loop either");
        fence.resetRetries(); // a real DOM request, attach, or resume
        check(fence.retry(), "New visible event restores one retry");
        check(!fence.retry(), "New event still has a finite retry budget");
        // Waiting/timeout alone never calls take(), so it cannot consume a refresh budget.
        DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
        policy.resume(0);
        check(policy.hasPending(), "A lost fence retains full-refresh intent");
        policy.request("page", 3000);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(3000));
    }

    private static void contentRefreshBudget() {
        DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
        policy.resume(0);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(0));
        for (int refresh = 1; refresh <= 30; refresh++) {
            long now = refresh * 400L;
            // Each visible data change may issue several notifications before its fence.
            for (int duplicate = 0; duplicate < 40; duplicate++) policy.request("content", now + duplicate);
            equal(DisplayRefreshPolicy.Update.NONE, policy.take(now + 119));
            equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(now + 120));
            equal(DisplayRefreshPolicy.Update.NONE, policy.take(now + 300));
        }
        // Thirty partials alone never create an unrequested clear on an idle display.
        check(!policy.hasPending(), "No periodic work without a visible change");
        equal(DisplayRefreshPolicy.Update.NONE, policy.take(100000));
        policy.request("content", 100400);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(100520));
        policy.request("content", 100800);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(100920));
    }

    private static void mixedRefreshBudget() {
        DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
        policy.resume(0);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(0));
        for (int page = 1; page <= 5; page++) {
            policy.request("page", page * 400L);
            equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(page * 400L));
        }
        for (int refresh = 6; refresh <= 30; refresh++) {
            policy.request("content", refresh * 400L);
            equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(refresh * 400L + 120));
        }
        policy.request("content", 12400);
        equal(DisplayRefreshPolicy.Update.FULL, policy.take(12520));
        // The budget-triggered clear also resets the independent page counter.
        policy.request("page", 12800);
        equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(12800));
    }

    private static void fullRefreshResetsBudget() {
        for (boolean resume : new boolean[]{false, true}) {
            DisplayRefreshPolicy policy = new DisplayRefreshPolicy();
            policy.resume(0);
            equal(DisplayRefreshPolicy.Update.FULL, policy.take(0));
            for (int refresh = 1; refresh <= 29; refresh++) {
                policy.request("content", refresh * 400L);
                equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(refresh * 400L + 120));
            }
            if (resume) {
                policy.request("content", 12000);
                policy.pause();
                for (int ignored = 0; ignored < 40; ignored++) {
                    check(!policy.request("content", 12000 + ignored), "Background requests do not count");
                    equal(DisplayRefreshPolicy.Update.NONE, policy.take(12200 + ignored));
                }
                policy.resume(12400);
            } else policy.request("full", 12400);
            equal(DisplayRefreshPolicy.Update.FULL, policy.take(12400));
            for (int refresh = 1; refresh <= 30; refresh++) {
                long now = 12400 + refresh * 400L;
                policy.request("content", now);
                equal(DisplayRefreshPolicy.Update.PARTIAL, policy.take(now + 120));
            }
            policy.request("content", 24800);
            equal(DisplayRefreshPolicy.Update.FULL, policy.take(24920));
        }
    }

    private static void equal(Object expected, Object actual) {
        if (!expected.equals(actual)) throw new AssertionError(expected + " != " + actual);
    }
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }
}
