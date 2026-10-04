package io.pluriview.viewer;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.function.LongSupplier;

/** Dedicated LAN socket: timeout polling never discards a partially read record. */
final class VideoConnection implements AutoCloseable {
    private final Socket socket = new Socket(Proxy.NO_PROXY);
    private final ResumableInput reader;
    private final DataInputStream input;
    final int status;
    VideoConnection(String base, String token, String client, Runnable waiting) throws IOException {
        this(base, token, client, waiting, "/v2/video", "application/x-pluriview-h264", 2_000_000_000L);
    }
    VideoConnection(String base, String token, String client, Runnable waiting, String path, String contentType, long responseTimeout) throws IOException {
        URL url = new URL(base);
        try {
            socket.setTcpNoDelay(true); socket.setSoTimeout(250);
            socket.connect(new InetSocketAddress(url.getHost(), url.getPort()), 1500);
            reader = new ResumableInput(socket.getInputStream(), System::nanoTime, waiting, responseTimeout);
            input = new DataInputStream(new BufferedInputStream(reader, 32768));
            socket.getOutputStream().write(("GET " + path + " HTTP/1.1\r\nHost: " + url.getHost() + ":" + url.getPort()
                + "\r\nAuthorization: Bearer " + token + "\r\nX-Pluriview-Client: " + client
                + "\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            reader.begin(true);
            ByteArrayOutputStream header = new ByteArrayOutputStream();
            int ending = 0; byte[] delimiter = {13,10,13,10};
            while (ending != 4) {
                int value = input.read();
                if (value < 0 || header.size() >= 4096) throw new IOException("Invalid video response");
                header.write(value);
                ending = value == delimiter[ending] ? ending + 1 : (value == 13 ? 1 : 0);
            }
            String[] lines = new String(header.toByteArray(), StandardCharsets.US_ASCII).split("\r\n");
            String[] first = lines[0].split(" ", 3);
            if (first.length < 2 || !"HTTP/1.1".equals(first[0]) || !first[1].matches("[0-9]{3}")) throw new IOException("Invalid video response");
            status = Integer.parseInt(first[1]);
            if (status == 200) {
                boolean type = false;
                for (int i = 1; i < lines.length; i++) {
                    int colon = lines[i].indexOf(':');
                    if (colon <= 0) throw new IOException("Invalid video response");
                    String name = lines[i].substring(0, colon).toLowerCase(Locale.ROOT);
                    String value = lines[i].substring(colon + 1).trim();
                    if (name.equals("content-type")) type = value.equals(contentType);
                    if (name.equals("transfer-encoding") || name.equals("content-encoding") || name.equals("content-length"))
                        throw new IOException("Unsupported video framing");
                }
                if (!type) throw new IOException("Invalid video response");
            }
        } catch (IOException | RuntimeException error) { close(); throw error; }
    }
    VideoPacket next() throws IOException { reader.begin(false); return new VideoPacket(input); }
    DataInputStream record() { reader.begin(false); return input; }
    @Override public void close() { try { socket.close(); } catch (IOException ignored) {} }

    static final class ResumableInput extends InputStream {
        private final InputStream source;
        private final LongSupplier clock;
        private final Runnable waiting;
        private long started, progress;
        private boolean header;
        private final long responseTimeout;
        ResumableInput(InputStream source, LongSupplier clock, Runnable waiting) {
            this(source, clock, waiting, 2_000_000_000L);
        }
        ResumableInput(InputStream source, LongSupplier clock, Runnable waiting, long responseTimeout) {
            this.source = source; this.clock = clock; this.waiting = waiting; this.responseTimeout = responseTimeout;
        }
        void begin(boolean header) { this.header = header; started = progress = clock.getAsLong(); }
        private void check() throws SocketTimeoutException {
            long now = clock.getAsLong();
            // Bound both silence and slow-drip records. No unbounded retry loop.
            if (now - started >= (header ? responseTimeout : 8_000_000_000L)
                || now - progress >= 5_000_000_000L)
                throw new SocketTimeoutException(header ? "Video response timeout" : "Video data timeout");
        }
        @Override public int read() throws IOException {
            byte[] one = new byte[1]; int n = read(one, 0, 1); return n < 0 ? -1 : one[0] & 255;
        }
        @Override public int read(byte[] bytes, int offset, int length) throws IOException {
            if (length == 0) return 0;
            while (true) {
                check();
                if (Thread.currentThread().isInterrupted()) throw new InterruptedIOException("Viewer stopped");
                try {
                    int count = source.read(bytes, offset, length);
                    if (count > 0) progress = clock.getAsLong();
                    return count;
                } catch (SocketTimeoutException timeout) {
                    check();
                    if (!header) waiting.run();
                }
            }
        }
    }
}
