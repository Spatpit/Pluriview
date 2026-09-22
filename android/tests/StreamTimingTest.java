package io.pluriview.viewer;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicReference;

public final class StreamTimingTest {
    private static void check(boolean value, String reason) { if (!value) throw new AssertionError(reason); }
    public static void main(String[] args) throws Exception {
        FramePacer pacer = new FramePacer();
        long previous = 0;
        // Deliver alternating pairs together, as a jittery network can do.
        for (int i = 0; i < 300; i++) {
            long arrival = 1_000_000_000L + (i / 2) * 66_666_000L;
            long due = pacer.due(i * 33333L, arrival);
            if (i > 0) check(due - previous == 33_333_000L, "Burst arrivals changed display cadence");
            check(due >= arrival && due - arrival <= 100_000_000L, "Unbounded playback delay");
            previous = due;
        }
        long afterStall = 20_000_000_000L;
        check(pacer.due(300 * 33333L, afterStall) == afterStall + 50_000_000L, "Stall did not reset playback clock");
        check(pacer.due(10_000_000L, afterStall + 33_333_000L) > afterStall, "Clock moved backwards");

        FrameRate rate = new FrameRate();
        check(rate.fps(0) == 0, "Empty counter claimed frames");
        for (int i = 0; i < 300; i++) rate.record(i * 33_333_333L);
        long now = 299 * 33_333_333L;
        check(rate.fps(now) == 30, "Steady rate or ring wrap failed");
        check(rate.fps(now + 500_000_000L) <= 15, "Stalled rate did not decay");
        check(rate.fps(now + 1_100_000_000L) == 0, "Stale frames reported as live");

        AtomicReference<Throwable> failure = new AtomicReference<>();
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            Thread peer = new Thread(() -> {
                try (Socket socket = server.accept()) {
                    socket.setSoTimeout(2000);
                    DataInputStream input = new DataInputStream(socket.getInputStream());
                    ByteArrayOutputStream header = new ByteArrayOutputStream();
                    while (!new String(header.toByteArray(), StandardCharsets.US_ASCII).endsWith("\r\n\r\n")) {
                        int b = input.read(); check(b >= 0 && header.size() < 4096, "Invalid request"); header.write(b);
                    }
                    check(new String(header.toByteArray(), StandardCharsets.US_ASCII).startsWith("GET /v2/control HTTP/1.1"), "Wrong control route");
                    OutputStream output = socket.getOutputStream();
                    output.write("HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-control\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
                    for (int i = 0; i < 20; i++) {
                        int length = input.readUnsignedShort(); check(length > 0 && length <= 1024, "Invalid message size");
                        byte[] body = new byte[length]; input.readFully(body);
                        String message = new String(body, StandardCharsets.UTF_8);
                        check(message.equals(i % 2 == 0 ? "{\"type\":\"camera\",\"value\":{\"scale\":1}}" : "{\"type\":\"ping\"}"), "Message framing corrupted");
                        // Split status bytes to exercise partial TCP reads.
                        output.write(0); output.flush(); output.write(i == 19 ? 201 : 200); output.flush();
                    }
                } catch (Throwable error) { failure.set(error); }
            });
            peer.start();
            try (ControlConnection control = new ControlConnection("http://127.0.0.1:" + server.getLocalPort(), "test-only", "test-client")) {
                for (int i = 0; i < 20; i++) check(control.send(i % 2 == 0 ? "camera" : "ping", i % 2 == 0 ? "{\"scale\":1}" : null) == (i == 19 ? 201 : 200), "Incorrect command acknowledgement");
            }
            peer.join(3000); check(!peer.isAlive(), "Control peer did not stop");
            if (failure.get() != null) throw new AssertionError("Control connection failed", failure.get());
        }
        System.out.println("Stream pacing, measured FPS and persistent control tests passed");
    }
}
