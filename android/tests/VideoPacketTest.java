package io.pluriview.viewer;

import java.io.*;

public final class VideoPacketTest {
    static byte[] record(int length, int width, int height, float viewport, int kind) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(bytes);
        out.writeInt(length); out.writeInt(width); out.writeInt(height);
        out.writeFloat(viewport); out.writeFloat(680); out.writeLong(42); out.writeInt(kind);
        if (length > 0 && length < 10) out.write(new byte[length]);
        return bytes.toByteArray();
    }
    static VideoPacket parse(byte[] bytes) throws IOException {
        return new VideoPacket(new DataInputStream(new ByteArrayInputStream(bytes)));
    }
    static void rejected(byte[] bytes) throws IOException {
        try { parse(bytes); throw new AssertionError("Accepted malformed packet"); }
        catch (IOException expected) {}
    }
    public static void main(String[] args) throws Exception {
        VideoPacket packet = parse(record(4, 1280, 680, 1280, 1));
        if (packet.sequence != 42 || packet.bytes.length != 4 || packet.viewportHeight != 680)
            throw new AssertionError("Wrong byte order");
        if (parse(record(0, 0, 0, 0, 2)).kind != 2) throw new AssertionError();
        if (parse(record(0, 0, 0, 0, 3)).kind != 3) throw new AssertionError();
        rejected(record(4 * 1024 * 1024 + 1, 1280, 680, 1280, 1));
        rejected(record(-1, 1280, 680, 1280, 1));
        rejected(record(4, 1281, 680, 1280, 1));
        rejected(record(4, 1280, 681, 1280, 1));
        rejected(record(4, 1280, 680, Float.NaN, 1));
        rejected(record(4, 1280, 680, 1280, 9));
        rejected(record(4, 0, 0, 0, 2));
        rejected(record(30, 1280, 680, 1280, 1));
        byte[] first = {0,0,0,1,0x67,0x42,0,0,1,0x68,0x33,0,0,1,0x65,0x55};
        H264Config config = new H264Config(first);
        if (!java.util.Arrays.equals(config.sps, new byte[]{0,0,0,1,0x67,0x42})
            || !java.util.Arrays.equals(config.pps, new byte[]{0,0,0,1,0x68,0x33})
            || !java.util.Arrays.equals(config.frame, new byte[]{0,0,1,0x65,0x55}))
            throw new AssertionError("Codec configuration must precede frame data");
        for (byte[] bad : new byte[][]{new byte[0], new byte[]{0,0,1}, new byte[]{0,0,1,0x41,0x55}}) {
            try { new H264Config(bad); throw new AssertionError("Missing keyframe accepted"); }
            catch (IOException expected) {}
        }
        if (args.length > 0) {
            H264Config actual = new H264Config(java.nio.file.Files.readAllBytes(java.nio.file.Paths.get(args[0])));
            if (actual.sps.length < 5 || actual.pps.length < 5) throw new AssertionError("Invalid system encoder configuration");
        }
        System.out.println("Video framing tests passed");
    }
}
