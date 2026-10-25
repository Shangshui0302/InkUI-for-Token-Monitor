package dev.tokenmonitor.eink;

/** Reflection behavior only: these fakes cannot establish a physical e-ink refresh. */
public final class OnyxDisplayBackendTest {
    public static void main(String[] args) {
        FakeView view = new FakeView();
        OnyxDisplayBackend backend = new OnyxDisplayBackend("ONYX", "Poke3", view, FakeView.class);
        check(backend.isSupported(), "Known target and complete interface");
        check(backend.refresh(false), "Partial request dispatched");
        equal(2, view.drawMode);
        equal(17, view.mode);
        check(backend.refresh(true), "Full request dispatched");
        equal(34, view.drawMode);
        equal(17, view.mode);
        check(!new OnyxDisplayBackend("Other", "Poke3", view, FakeView.class).isSupported(), "Other vendor rejected");
        check(!new OnyxDisplayBackend("ONYX", "Other", view, FakeView.class).isSupported(), "Unproven model rejected");
        check(!new OnyxDisplayBackend("ONYX", "Poke3", new Object(), Object.class).isSupported(), "Absent interface safe");
        view.failDraw = true;
        check(!backend.refresh(true), "Invocation failure falls back");
        check(!backend.isSupported(), "Runtime failure permanently downgrades support");
        check("system-redraw".equals(backend.method()), "Fallback accurately named");
        equal(17, view.mode);
        int calls = view.drawCalls;
        check(!backend.refresh(false), "Failed backend is not retried");
        equal(calls, view.drawCalls);
        FakeView denied = new FakeView();
        denied.failSet = true;
        backend = new OnyxDisplayBackend("ONYX", "Poke3", denied, FakeView.class);
        check(!backend.refresh(true), "Access failure does not escape");
        check(!backend.isSupported(), "Access failure downgrades");
        equal(0, denied.drawCalls);
        FakeView restoreFailure = new FakeView();
        restoreFailure.failRestore = true;
        backend = new OnyxDisplayBackend("ONYX", "Poke3", restoreFailure, FakeView.class);
        check(!backend.refresh(true), "Restoration failure downgrades");
        check(!backend.isSupported(), "Restoration error is reflected in capabilities");
        System.out.println("Onyx display adapter checks passed");
    }

    public static final class FakeView {
        int mode = 17;
        int drawMode;
        int drawCalls;
        boolean failDraw;
        boolean failSet;
        boolean failRestore;
        public int getDefaultUpdateMode() { return mode; }
        public void setDefaultUpdateMode(int value) {
            if (failSet || (failRestore && value == 17)) throw new SecurityException("Unavailable");
            mode = value;
        }
        public void invalidate() {
            drawCalls++;
            if (failDraw) throw new IllegalStateException("Unavailable");
            drawMode = mode;
        }
    }
    private static void equal(int expected, int actual) {
        if (expected != actual) throw new AssertionError(expected + " != " + actual);
    }
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }
}
