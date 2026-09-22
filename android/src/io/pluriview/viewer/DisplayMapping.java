package io.pluriview.viewer;

/** Full-frame rectangle, including the invisible edges in Fill mode. */
public final class DisplayMapping {
    public final float left, top, width, height;
    public DisplayMapping(float viewWidth, float viewHeight, float imageWidth, float imageHeight, boolean fill) {
        if (viewWidth <= 0 || viewHeight <= 0 || imageWidth <= 0 || imageHeight <= 0)
            throw new IllegalArgumentException("Empty display");
        float x = viewWidth / imageWidth, y = viewHeight / imageHeight;
        float scale = fill ? Math.max(x, y) : Math.min(x, y);
        width = imageWidth * scale; height = imageHeight * scale;
        left = (viewWidth - width) / 2; top = (viewHeight - height) / 2;
    }
    public float x(float screenX) { return (screenX - left) / width; }
    public float y(float screenY) { return (screenY - top) / height; }
}
