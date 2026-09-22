import io.pluriview.viewer.CameraTransform;

public class CameraTransformTest {
    static void near(float value, float expected) {
        if (Math.abs(value - expected) > .00001f) throw new AssertionError(value + " != " + expected);
    }
    public static void main(String[] args) {
        CameraTransform camera = new CameraTransform();
        // Two coalesced gestures must equal applying each gesture in order.
        camera.append(1, .1f, -.2f);
        camera.append(2, CameraTransform.anchoredTranslation(.5f, .5f, 2),
            CameraTransform.anchoredTranslation(.5f, .5f, 2));
        float[] result = camera.take();
        near(result[0], 2); near(result[1], -.3f); near(result[2], -.9f);
        if (camera.take() != null) throw new AssertionError("Gesture replayed");
        camera.append(Float.NaN, 0, 0);
        if (camera.take() != null) throw new AssertionError("Invalid transform accepted");
        camera.append(1, .2f, .1f); camera.clear();
        if (camera.take() != null) throw new AssertionError("Cancelled gesture replayed");
        camera.append(2, -.5f, -.75f, .5f, .75f);
        result = camera.take();
        near(result[1], 0); near(result[2], 0);
        near(result[3], .5f); near(result[4], .75f);
        System.out.println("Camera transform tests passed");
    }
}
