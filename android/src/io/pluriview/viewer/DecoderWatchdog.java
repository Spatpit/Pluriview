package io.pluriview.viewer;

/** Only outstanding decoder work consumes the stall deadline. */
final class DecoderWatchdog {
    private boolean waiting;
    private long progress;
    boolean stalled(long now, boolean pending, boolean advanced) {
        if (!pending) { waiting = false; return false; }
        if (!waiting || advanced) { waiting = true; progress = now; }
        return now - progress > 750_000_000L;
    }
}
