package io.pluriview.viewer;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

public final class AudioStartupTest {
    private static void check(boolean value, String message) { if (!value) throw new AssertionError(message); }
    private static void stopped(DecoderSlot slot) throws Exception {
        long deadline = System.nanoTime() + 1_000_000_000L;
        while (!slot.available() && System.nanoTime() < deadline) Thread.sleep(5);
        check(slot.available(), "Audio worker survived toggle-off");
    }
    public static void main(String[] args) throws Exception {
        AtomicReference<Throwable> failure = new AtomicReference<>();
        DecoderSlot network = new DecoderSlot(), codec = new DecoderSlot();
        CountDownLatch accepted = new CountDownLatch(1);
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            Thread sender = new Thread(() -> {
                try (Socket socket = server.accept()) {
                    accepted.countDown(); socket.setSoTimeout(3000);
                    BufferedReader request = new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII));
                    String line;
                    do { line = request.readLine(); check(line != null, "Missing request"); } while (!line.isEmpty());
                    DataOutputStream out = new DataOutputStream(socket.getOutputStream());
                    out.writeBytes("HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-aac\r\nConnection: close\r\n\r\n");
                    out.writeInt(0x50564131); out.writeShort(0x1190);
                    out.writeInt(1); out.writeByte(0); out.flush();
                    check(socket.getInputStream().read() == -1, "Toggle-off left transport open");
                } catch (Throwable error) { failure.set(error); }
            });
            sender.start();
            try (AudioStream stream = new AudioStream("http://127.0.0.1:" + server.getLocalPort(), "fixture", "client", network, codec, text -> {})) {
                // Longer than the six-packet (128 ms) live buffer: streaming
                // must not begin until slow device initialization completes.
                check(!accepted.await(400, TimeUnit.MILLISECONDS), "PC capture started before decoder readiness");
                AudioDecoder.prepare.countDown();
                check(accepted.await(1, TimeUnit.SECONDS), "Prepared decoder did not connect");
                long deadline = System.nanoTime() + 1_000_000_000L;
                while (AudioDecoder.received.get() == 0 && System.nanoTime() < deadline) Thread.sleep(5);
                check(AudioDecoder.received.get() == 1, "Prepared stream lost audio");
            } finally { AudioDecoder.prepare.countDown(); }
            sender.join(3000); check(!sender.isAlive(), "Sender did not stop");
            if (failure.get() != null) throw new AssertionError(failure.get());
        }
        stopped(network); stopped(codec);

        // Toggle-off during initialization is also bounded and starts no socket.
        AudioDecoder.prepare = new CountDownLatch(1);
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            AudioStream stream = new AudioStream("http://127.0.0.1:" + server.getLocalPort(), "fixture", "client", network, codec, text -> {});
            Thread.sleep(40); stream.close(); stopped(network); stopped(codec);
            server.setSoTimeout(100);
            try { server.accept().close(); throw new AssertionError("Cancelled startup connected"); }
            catch (SocketTimeoutException expected) {}
        } finally { AudioDecoder.prepare.countDown(); }

        AudioStartup failed = new AudioStartup();
        failed.finish(new IOException("Audio decoder unavailable"));
        try { failed.await(); throw new AssertionError("Startup failure was hidden"); }
        catch (IOException expected) { check("Decoder unavailable".equals(AudioStream.failureReason(expected)), "Wrong failure label"); }
        long start = System.nanoTime();
        try { new AudioStartup().await(); throw new AssertionError("Stuck startup waited forever"); }
        catch (IOException expected) { check("Decoder startup timed out".equals(AudioStream.failureReason(expected)), "Wrong timeout label"); }
        check(System.nanoTime() - start < 4_000_000_000L, "Unbounded startup wait");
        check("PC ended audio".equals(AudioStream.failureReason(new EOFException())), "Missing server disconnect reason");
        check("Audio playback interrupted".equals(AudioStream.failureReason(new IOException("private-token-fixture"))), "Raw error reached the UI");
        System.out.println("Audio delayed startup, live transport ordering, cancellation, deadlines and safe status tests passed");
    }
}
