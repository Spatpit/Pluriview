package io.pluriview.viewer;

/** Normalized screen transform; independent of transport and target camera. */
public final class CameraTransform {
    private float scale = 1, x = 0, y = 0;
    private float anchorX = 0, anchorY = 0;

    public synchronized void append(float factor, float dx, float dy) {
        append(factor, dx, dy, 0, 0);
    }

    public synchronized void append(float factor, float dx, float dy, float ax, float ay) {
        float nextScale = scale * factor;
        float nextX = x * factor + dx;
        float nextY = y * factor + dy;
        if (!Float.isFinite(nextScale) || nextScale < .25f || nextScale > 4f
                || !Float.isFinite(nextX) || !Float.isFinite(nextY)
                || Math.abs(nextX) > 2 || Math.abs(nextY) > 2) return;
        scale = nextScale; x = nextX; y = nextY;
        anchorX = Math.max(0, Math.min(1, ax)); anchorY = Math.max(0, Math.min(1, ay));
    }

    public synchronized float[] take() {
        if (scale == 1 && x == 0 && y == 0) return null;
        // Retain a real pinch anchor in the protocol. A top-left-only affine
        // transform would accidentally pan when the PC clamps its zoom limit.
        float[] result = {scale, x + (scale - 1) * anchorX, y + (scale - 1) * anchorY, anchorX, anchorY};
        clear();
        return result;
    }

    public synchronized void clear() { scale = 1; x = 0; y = 0; anchorX = 0; anchorY = 0; }

    /** Translation that holds a pinch's old midpoint under its new midpoint. */
    public static float anchoredTranslation(float oldPosition, float newPosition, float scale) {
        return newPosition - scale * oldPosition;
    }
}
