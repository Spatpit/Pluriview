package io.pluriview.viewer;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;

/** Small authenticated messages over one socket; uncertain commands are never retried. */
final class ControlConnection implements AutoCloseable {
    static final class Unsupported extends IOException {}
    private final Socket socket = new Socket(Proxy.NO_PROXY);
    private final DataInputStream input;
    private final DataOutputStream output;
    long lastSend;
    ControlConnection(String base, String token, String client) throws IOException {
        URL url = new URL(base);
        try {
            socket.setTcpNoDelay(true); socket.setSoTimeout(1000);
            socket.connect(new InetSocketAddress(url.getHost(), url.getPort()), 1000);
            input = new DataInputStream(new BufferedInputStream(socket.getInputStream(), 1024));
            output = new DataOutputStream(socket.getOutputStream());
            output.write(("GET /v2/control HTTP/1.1\r\nHost: " + url.getHost() + "\r\nAuthorization: " + Auth.header(token, "GET", "/v2/control", client) + "\r\nX-Pluriview-Client: " + client + "\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            ByteArrayOutputStream header = new ByteArrayOutputStream(); int ending = 0;
            byte[] delimiter = {13,10,13,10};
            while (ending != 4) {
                int value = input.read(); if (value < 0 || header.size() >= 4096) throw new IOException("Invalid control response");
                header.write(value);
                ending = value == delimiter[ending] ? ending + 1 : (value == 13 ? 1 : 0);
            }
            String response = new String(header.toByteArray(), StandardCharsets.US_ASCII);
            if (response.startsWith("HTTP/1.1 404")) throw new Unsupported();
            if (!response.startsWith("HTTP/1.1 200") || !response.contains("application/x-pluriview-control")) throw new IOException("Control connection unavailable");
            lastSend = System.nanoTime();
        } catch (IOException | RuntimeException error) { close(); throw error; }
    }
    int send(String type, String body) throws IOException {
        byte[] bytes = (body == null ? "{\"type\":\"ping\"}" : "{\"type\":\"" + type + "\",\"value\":" + body + "}").getBytes(StandardCharsets.UTF_8);
        if (bytes.length > 1024) throw new IOException("Control message too large");
        ByteArrayOutputStream framed = new ByteArrayOutputStream(bytes.length + 2);
        DataOutputStream record = new DataOutputStream(framed); record.writeShort(bytes.length); record.write(bytes);
        output.write(framed.toByteArray());
        int status = input.readUnsignedShort(); lastSend = System.nanoTime(); return status;
    }
    @Override public void close() { try { socket.close(); } catch (IOException ignored) {} }
}
