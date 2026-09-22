import io.pluriview.viewer.DisplayMapping;

public final class DisplayMappingTest {
    static void near(float a, float b) { if (Math.abs(a - b) > .001) throw new AssertionError(a + " != " + b); }
    public static void main(String[] args) {
        DisplayMapping fit = new DisplayMapping(2400, 1080, 1920, 1080, false);
        near(fit.left, 240); near(fit.top, 0); near(fit.x(240), 0); near(fit.x(2160), 1);
        DisplayMapping fill = new DisplayMapping(2400, 1080, 1920, 1080, true);
        near(fill.left, 0); near(fill.top, -135); near(fill.y(0), .1f); near(fill.y(1080), .9f);
        near(fill.x(1200), .5f); near(fill.y(540), .5f);
        DisplayMapping portrait = new DisplayMapping(1080, 2400, 1920, 1080, true);
        near(portrait.x(540), .5f); near(portrait.y(1200), .5f);
        // A drag maps through the full PC frame, not just the visible crop.
        near(portrait.x(640) - portrait.x(540), 100f / portrait.width);
        DisplayMapping rotated = new DisplayMapping(2400, 1080, 1920, 1080, true);
        near(rotated.width / rotated.height, 1920f / 1080);
        System.out.println("Fit/Fill mapping tests passed");
    }
}
