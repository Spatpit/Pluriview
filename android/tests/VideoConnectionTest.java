package io.pluriview.viewer;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

public final class VideoConnectionTest {
    private static void check(boolean value, String reason) { if (!value) throw new AssertionError(reason); }
    public static void main(String[] args) throws Exception {
        byte[] record = VideoPacketTest.record(4, 1280, 680, 1280, 1);
        AtomicInteger waits = new AtomicInteger(); AtomicReference<Throwable> failure = new AtomicReference<>();
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            Thread sender = new Thread(() -> {
                try (Socket socket = server.accept()) {
                    socket.setSoTimeout(3000); InputStream input = socket.getInputStream();
                    ByteArrayOutputStream request = new ByteArrayOutputStream();
                    while (!new String(request.toByteArray(), StandardCharsets.US_ASCII).endsWith("\r\n\r\n")) {
                        int b = input.read(); check(b >= 0 && request.size() < 4096, "Invalid request"); request.write(b);
                    }
                    check(new String(request.toByteArray(), StandardCharsets.US_ASCII).startsWith("GET /v2/video HTTP/1.1"), "Wrong video route");
                    OutputStream output = socket.getOutputStream();
                    output.write("HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-h264\r\nConnection: close\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
                    output.write(record, 0, 2); output.flush();
                    // A timeout halfway through readInt used to discard the
                    // entire TCP stream and decoder. Keep the same connection.
                    Thread.sleep(2300);
                    output.write(record, 2, 31); output.flush();
                    Thread.sleep(400);
                    output.write(record, 33, record.length - 33); output.write(record); output.flush();
                    while (input.read() != -1) {}
                } catch (Throwable error) { failure.set(error); }
            }); sender.start();
            try (VideoConnection video = new VideoConnection("http://127.0.0.1:" + server.getLocalPort(), "test-only", "test-client", waits::incrementAndGet)) {
                check(video.status == 200, "Handshake failed");
                VideoPacket first = video.next(), second = video.next();
                check(Arrays.equals(first.bytes, second.bytes) && first.sequence == second.sequence, "Partial timeout corrupted framing");
                check(waits.get() >= 2, "Interrupted delivery was not exercised");
                Thread blocked = new Thread(() -> { try { video.next(); failure.set(new AssertionError("Closed socket read succeeded")); } catch (IOException expected) {} });
                blocked.start(); Thread.sleep(100); video.close(); blocked.join(1000);
                check(!blocked.isAlive(), "Background close did not cancel network read");
            }
            sender.join(3000); check(!sender.isAlive(), "Sender leaked");
            if (failure.get() != null) throw new AssertionError(failure.get());
        }
        AtomicLong now = new AtomicLong();
        InputStream silent = new InputStream() { public int read() throws IOException { now.addAndGet(250_000_000L); throw new SocketTimeoutException(); } };
        VideoConnection.ResumableInput reader = new VideoConnection.ResumableInput(silent, now::get, () -> {});
        reader.begin(false);
        try { reader.read(); throw new AssertionError("Infinite silence was accepted"); }
        catch (SocketTimeoutException expected) { check(now.get() == 5_000_000_000L, "Silence deadline not bounded"); }
        now.set(0); reader.begin(true);
        try { reader.read(); throw new AssertionError("Stuck handshake accepted"); }
        catch (SocketTimeoutException expected) { check(now.get() == 2_000_000_000L, "Handshake deadline changed"); }
        now.set(0);
        reader = new VideoConnection.ResumableInput(new InputStream() { public int read() { now.addAndGet(1_000_000_000L); return 0; } }, now::get, () -> {});
        reader.begin(false);
        try { for (int i = 0; i < 10; i++) reader.read(); throw new AssertionError("Slow-drip record accepted forever"); }
        catch (SocketTimeoutException expected) { check(now.get() == 8_000_000_000L, "Total record deadline not enforced"); }
        System.out.println("Video partial-timeout recovery, bounded reads and cancellation tests passed");
    }
}
