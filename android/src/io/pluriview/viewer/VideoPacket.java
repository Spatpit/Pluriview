package io.pluriview.viewer;

import java.io.DataInputStream;
import java.io.IOException;

/** Bounded framing for one Annex-B H.264 access unit; network byte order. */
final class VideoPacket {
    final byte[] bytes;
    final int width, height, kind;
    final float viewportWidth, viewportHeight;
    final long sequence;

    VideoPacket(DataInputStream input) throws IOException {
        int length = input.readInt();
        width = input.readInt(); height = input.readInt();
        viewportWidth = input.readFloat(); viewportHeight = input.readFloat();
        sequence = input.readLong(); kind = input.readInt();
        if (kind < 1 || kind > 3 || length < 0 || length > 4 * 1024 * 1024)
            throw new IOException("Invalid video record");
        if (kind == 1) {
            if (length == 0 || width < 2 || width > 1280 || height < 2 || height > 720
                || (width & 1) != 0 || (height & 1) != 0 || sequence <= 0
                || !Float.isFinite(viewportWidth) || !Float.isFinite(viewportHeight)
                || viewportWidth < 1 || viewportWidth > 32768 || viewportHeight < 1 || viewportHeight > 32768)
                throw new IOException("Invalid video dimensions");
        } else if (length != 0) { throw new IOException("Invalid status record"); }
        bytes = new byte[length]; input.readFully(bytes);
    }
}
