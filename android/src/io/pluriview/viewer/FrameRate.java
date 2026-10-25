package io.pluriview.viewer;

/** Count observed events, not requested FPS. Also decays to zero during stalls. */
public final class FrameRate {
    private final long[] times = new long[120];
    private int next, count;
    public synchronized void clear() { next = 0; count = 0; }
    public synchronized void record(long now) {
        times[next] = now; next = (next + 1) % times.length; count = Math.min(count + 1, times.length);
    }
    public synchronized int fps(long now) {
        int recent = 0; long oldest = now;
        for (int i = 0; i < count; i++) {
            long time = times[(next - 1 - i + times.length) % times.length];
            if (now - time > 1_000_000_000L) break;
            recent++; oldest = time;
        }
        if (recent < 2) return 0;
        return (int) Math.round((recent - 1) * 1_000_000_000.0 / Math.max(1, now - oldest));
    }
}
