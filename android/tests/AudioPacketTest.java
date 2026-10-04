package io.pluriview.viewer;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicReference;

public final class AudioPacketTest {
    private static void check(boolean value, String reason) { if (!value) throw new AssertionError(reason); }
    private static DataInputStream input(byte[] data) { return new DataInputStream(new ByteArrayInputStream(data)); }
    private static byte[] record(int size) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream(); DataOutputStream data = new DataOutputStream(out);
        data.writeInt(size); if (size > 0 && size <= 8192) data.write(new byte[size]); return out.toByteArray();
    }
    public static void main(String[] args) throws Exception {
        byte[] header = {0x50, 0x56, 0x41, 0x31, 0x11, (byte) 0x90};
        AudioPacket.header(input(header));
        for (int i = 0; i < header.length; i++) {
            byte[] wrong = header.clone(); wrong[i] ^= 1;
            try { AudioPacket.header(input(wrong)); throw new AssertionError("Invalid format accepted"); } catch (IOException expected) {}
        }
        for (int length : new int[] {0, -1, 8193, Integer.MAX_VALUE}) {
            try { new AudioPacket(input(record(length))); throw new AssertionError("Unbounded packet accepted"); } catch (IOException expected) {}
        }
        check(new AudioPacket(input(record(8192))).bytes.length == 8192, "Maximum packet rejected");
        byte[] packet = record(30);
        for (int i = 0; i < packet.length; i++) {
            try { new AudioPacket(input(Arrays.copyOf(packet, i))); throw new AssertionError("Truncated packet accepted"); } catch (IOException expected) {}
        }
        AtomicReference<Throwable> failure = new AtomicReference<>();
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            Thread sender = new Thread(() -> {
                try (Socket socket = server.accept()) {
                    socket.setSoTimeout(3000); ByteArrayOutputStream request = new ByteArrayOutputStream();
                    InputStream in = socket.getInputStream();
                    while (!new String(request.toByteArray(), StandardCharsets.US_ASCII).endsWith("\r\n\r\n")) {
                        int value = in.read(); check(value >= 0 && request.size() < 4096, "Invalid request"); request.write(value);
                    }
                    String text = new String(request.toByteArray(), StandardCharsets.US_ASCII);
                    check(text.startsWith("GET /v2/audio HTTP/1.1"), "Wrong audio route");
                    check(text.contains("Authorization: Bearer test-only") && text.contains("X-Pluriview-Client: test-client"), "Missing authentication");
                    OutputStream out = socket.getOutputStream();
                    out.write("HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-aac\r\nConnection: close\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
                    out.write(header); out.write(packet, 0, 2); out.flush(); Thread.sleep(300);
                    out.write(packet, 2, packet.length - 2); out.flush();
                    check(in.read() == -1, "Audio off did not close the socket");
                } catch (Throwable error) { failure.set(error); }
            }); sender.start();
            try (VideoConnection audio = new VideoConnection("http://127.0.0.1:" + server.getLocalPort(), "test-only", "test-client", () -> {}, "/v2/audio", "application/x-pluriview-aac", 6_000_000_000L)) {
                check(audio.status == 200, "Audio handshake failed"); AudioPacket.header(audio.record());
                check(new AudioPacket(audio.record()).bytes.length == 30, "Interrupted audio framing lost");
                Thread blocked = new Thread(() -> { try { new AudioPacket(audio.record()); failure.set(new AssertionError("Cancelled read succeeded")); } catch (IOException expected) {} });
                blocked.start(); Thread.sleep(50); audio.close(); blocked.join(1000);
                check(!blocked.isAlive(), "Audio read survived toggle-off");
            }
            sender.join(3000); check(!sender.isAlive(), "Audio sender leaked");
            if (failure.get() != null) throw new AssertionError(failure.get());
        }
        System.out.println("Audio format, bounds, partial reads, authentication headers and toggle-off cancellation tests passed");
    }
}
