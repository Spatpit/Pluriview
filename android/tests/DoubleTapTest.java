import io.pluriview.viewer.DoubleTap;

public final class DoubleTapTest {
    public static void main(String[] args) {
        DoubleTap tap = new DoubleTap(300, 500, 40);
        if (tap.up(0, 50, 10, 10) || !tap.up(120, 160, 12, 10)) throw new AssertionError("Nearby taps must focus");
        if (tap.up(200, 240, 12, 10)) throw new AssertionError("Triple tap must not toggle twice");
        tap.clear(); // A pinch, drag, resize or pause cancels the first tap.
        if (tap.up(280, 300, 12, 10)) throw new AssertionError("Cancelled tap reused");
        if (tap.up(320, 350, 100, 100)) throw new AssertionError("Distant tiles paired");
        if (tap.up(400, 1000, 100, 100)) throw new AssertionError("Long press paired");
        if (tap.up(1010, 1050, 100, 100)) throw new AssertionError("Long press retained");
        if (tap.up(1500, 1540, 100, 100)) throw new AssertionError("Expired tap paired");
        System.out.println("Double-tap tests passed");
    }
}
