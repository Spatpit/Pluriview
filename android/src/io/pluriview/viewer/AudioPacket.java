package io.pluriview.viewer;

import java.io.DataInputStream;
import java.io.IOException;

/** Fixed AAC-LC stereo profile; framing is bounded before any allocation. */
final class AudioPacket {
    static void header(DataInputStream in) throws IOException {
        if (in.readInt() != 0x50564131 || in.readUnsignedShort() != 0x1190)
            throw new IOException("Unsupported audio format");
    }
    final byte[] bytes;
    final long received = System.nanoTime();
    AudioPacket(DataInputStream in) throws IOException {
        int length = in.readInt();
        if (length < 1 || length > 8192) throw new IOException("Invalid audio packet");
        bytes = new byte[length]; in.readFully(bytes);
    }
}
