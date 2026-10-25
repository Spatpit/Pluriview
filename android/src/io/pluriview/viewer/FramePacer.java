package io.pluriview.viewer;

/** A small, bounded playout cushion for bursty LAN delivery. Times are nanoseconds. */
public final class FramePacer {
    private long origin = Long.MIN_VALUE;
    public long due(long presentationUs, long now) {
        if (origin == Long.MIN_VALUE) origin = now + 50_000_000L - presentationUs * 1000;
        long due = origin + presentationUs * 1000;
        if (due < now - 100_000_000L || due > now + 150_000_000L) {
            origin = now + 50_000_000L - presentationUs * 1000;
            due = now + 50_000_000L;
        }
        return due;
    }
}
