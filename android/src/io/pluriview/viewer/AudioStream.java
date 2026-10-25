package io.pluriview.viewer;

import java.io.IOException;
import java.util.function.Consumer;

/** Independently cancellable audio receiver; errors never restart video. */
final class AudioStream implements AutoCloseable {
    private final Thread worker;
    private volatile boolean closed;
    private volatile VideoConnection connection;
    private volatile AudioDecoder decoder;
    AudioStream(String base, String token, String client, DecoderSlot networkSlot, DecoderSlot codecSlot, Consumer<String> status) throws IOException {
        worker = new Thread(() -> {
            String[] last = {""};
            receive(base, token, client, codecSlot, value -> {
                if (!last[0].equals(value)) { last[0] = value; status.accept(value); }
            });
        }, "viewer-audio");
        networkSlot.start(worker);
    }
    private void receive(String base, String token, String client, DecoderSlot slot, Consumer<String> status) {
        String previousFailure = "";
        while (!closed) {
            int delay = 1000;
            try {
                if (!slot.available()) { status.accept("Audio: waiting for decoder"); Thread.sleep(250); continue; }
                status.accept("Audio: preparing…" + previousFailure);
                // Starting after the HTTP response allowed the PC to fill the
                // six-packet live queue while MediaCodec/AudioTrack initialized.
                // Keep startup outside the live stream, without a larger backlog.
                try (AudioDecoder d = new AudioDecoder(slot)) {
                    decoder = d;
                    d.awaitReady();
                    if (closed) break;
                    status.accept("Audio: connecting…" + previousFailure);
                    try (VideoConnection c = new VideoConnection(base, token, client, () -> {
                        status.accept("Audio: waiting for data");
                    }, "/v2/audio", "application/x-pluriview-aac", 6_000_000_000L)) {
                        connection = c;
                        if (closed) break;
                        if (c.status != 200) {
                            delay = 3000;
                            if (c.status == 404) { status.accept("Audio needs the new PC build"); return; }
                            if (c.status == 401) { status.accept("Audio: pair again on the PC"); return; }
                            if (c.status == 412) status.accept("Audio: update the PC app for desktop monitoring support");
                            else if (c.status == 423 || c.status == 409) status.accept("Audio: waiting for canvas");
                            else status.accept("Audio unavailable on this PC");
                        } else {
                            AudioPacket.header(c.record());
                            while (!closed) {
                                AudioPacket packet = new AudioPacket(c.record());
                                if (closed) break;
                                d.queue(packet); status.accept("Audio: on · stereo");
                            }
                        }
                    } finally { connection = null; }
                } finally { decoder = null; }
            } catch (Exception error) {
                previousFailure = " · " + failureReason(error);
                if (!closed) status.accept("Audio: reconnecting" + previousFailure);
            }
            if (!closed) { try { Thread.sleep(delay); } catch (InterruptedException stopped) { break; } }
        }
    }
    static String failureReason(Exception error) {
        // Only fixed labels reach the UI, never driver errors, URLs or tokens.
        if (error instanceof java.net.SocketTimeoutException) return "Connection timed out";
        if (error instanceof java.io.EOFException) return "PC ended audio";
        if (error instanceof java.net.SocketException) return "Connection interrupted";
        String detail = error.getMessage();
        if ("Audio decoder startup timed out".equals(detail)) return "Decoder startup timed out";
        if ("Audio decoder unavailable".equals(detail)) return "Decoder unavailable";
        if ("Audio output unavailable".equals(detail)) return "Phone audio output unavailable";
        if ("Audio backlog".equals(detail)) return "Audio queue full";
        if ("Audio input queue stalled".equals(detail)) return "Decoder not accepting audio";
        if ("Audio decoder stalled".equals(detail)) return "Decoder stalled";
        if ("Audio output stalled".equals(detail)) return "Phone audio output stalled";
        if ("Unsupported decoded audio".equals(detail) || "Unsupported audio format".equals(detail)) return "Unsupported audio format";
        return "Audio playback interrupted";
    }
    @Override public void close() {
        closed = true; worker.interrupt();
        VideoConnection c = connection; if (c != null) c.close();
        AudioDecoder d = decoder; if (d != null) d.close();
    }
}
