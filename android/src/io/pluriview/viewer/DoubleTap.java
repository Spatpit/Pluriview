package io.pluriview.viewer;

/** Recognize two completed taps, never the second finger of a pinch. */
public final class DoubleTap {
    private final long gap, press;
    private final float distance;
    private long previousUp = -1;
    private float previousX, previousY;
    public DoubleTap(long gap, long press, float distance) {
        this.gap = gap; this.press = press; this.distance = distance;
    }
    public void clear() { previousUp = -1; }
    public boolean up(long down, long up, float x, float y) {
        if (up < down || up - down > press) { clear(); return false; }
        boolean paired = previousUp >= 0 && down >= previousUp && down - previousUp <= gap
            && Math.hypot(x - previousX, y - previousY) <= distance;
        if (paired) clear();
        else { previousUp = up; previousX = x; previousY = y; }
        return paired;
    }
}
