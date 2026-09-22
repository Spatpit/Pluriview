package io.pluriview.viewer;

/** Brief reconnects retain the last texture, but never indefinitely. */
final class RecoveryWindow {
    static final long HOLD_NANOS = 1_000_000_000L;
    private boolean recovering;
    private long started;
    synchronized boolean begin(long now) {
        if (recovering) return false;
        recovering = true; started = now; return true;
    }
    synchronized void reset() { recovering = false; }
    synchronized boolean expired(long now) { return recovering && now - started >= HOLD_NANOS; }
}
