package io.pluriview.viewer;

import android.app.Activity;
import android.graphics.Color;
import android.os.Build;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.UUID;
import java.util.concurrent.*;
import org.json.JSONObject;

/** Nearby discovery never obtains viewer access until the PC owner approves. */
final class PairingScreen {
    interface Connected { void connect(String ip, int port, String token, String client); }
    private final Activity activity;
    private final Connected connected;
    private final LinearLayout rows;
    private final TextView status;
    private final Button cancel;
    private final ConcurrentHashMap<String, Host> hosts = new ConcurrentHashMap<>();
    private final String client = UUID.randomUUID().toString().replace("-", "");
    private NearbyDiscovery discovery;
    private ScheduledExecutorService worker;
    private ScheduledFuture<?> timer;
    private volatile HttpURLConnection connection;
    private volatile Attempt pending;
    private volatile int generation;
    private boolean running;

    private static final class Host {
        final String key, ip; final int port;
        volatile String name;
        volatile boolean busy, known;
        volatile long checked;
        Host(String key, String ip, int port) { this.key = key; this.ip = ip; this.port = port; }
    }
    private static final class Attempt {
        final Host host; final String request = UUID.randomUUID().toString().replace("-", "");
        final long started = System.nanoTime();
        Attempt(Host host) { this.host = host; }
    }
    private static final class Reply {
        final int code; final String body;
        Reply(int code, String body) { this.code = code; this.body = body; }
    }

    PairingScreen(Activity activity, LinearLayout root, Connected connected) {
        this.activity = activity; this.connected = connected;
        status = new TextView(activity); status.setTextColor(Color.WHITE); status.setText("Looking for nearby PCs…"); root.addView(status);
        rows = new LinearLayout(activity); rows.setOrientation(LinearLayout.VERTICAL); root.addView(rows);
        cancel = new Button(activity); cancel.setText("Cancel connection request"); cancel.setVisibility(android.view.View.GONE); root.addView(cancel);
        cancel.setOnClickListener(v -> cancelRequest());
        Button refresh = new Button(activity); refresh.setText("Refresh nearby PCs"); root.addView(refresh);
        refresh.setOnClickListener(v -> { stop(); start(); });
    }
    void start() {
        if (running) return;
        running = true; final int run = ++generation;
        status.setText("Looking for PCs sharing on your local network…"); hosts.clear(); render();
        worker = Executors.newSingleThreadScheduledExecutor();
        discovery = new NearbyDiscovery(activity, new NearbyDiscovery.Listener() {
            public void found(String key, String ip, int port) {
                if (generation == run && hosts.size() < 16) { hosts.put(key, new Host(key, ip, port)); }
            }
            public void lost(String key) { if (generation == run) { hosts.remove(key); render(); } }
            public void unavailable() { if (generation == run) status.setText("Local discovery unavailable. Use the QR/link below, or refresh."); }
        });
        timer = worker.scheduleWithFixedDelay(() -> tick(run), 0, 750, TimeUnit.MILLISECONDS);
    }
    void stop() {
        if (!running) return;
        running = false; generation++;
        if (discovery != null) discovery.close();
        if (timer != null) timer.cancel(false);
        HttpURLConnection active = connection; if (active != null) active.disconnect();
        Attempt previous = pending; pending = null;
        if (previous != null) worker.execute(() -> cancelRemote(previous));
        worker.shutdown();
        cancel.setVisibility(android.view.View.GONE);
    }
    private void cancelRequest() {
        Attempt previous = pending; pending = null;
        if (previous != null) worker.execute(() -> cancelRemote(previous));
        status.setText("Request cancelled. You can choose a PC again."); render();
    }
    private void cancelRemote(Attempt attempt) { try { request(attempt.host, "/v3/pair/cancel", body(attempt), false); } catch (Exception ignored) {} }
    private String body(Attempt attempt) throws Exception {
        String name = Build.MODEL.replaceAll("[^A-Za-z0-9 _().-]", "").trim();
        if (name.isEmpty()) name = "Android phone";
        return new JSONObject().put("request", attempt.request).put("name", name.substring(0, Math.min(64, name.length()))).toString();
    }
    private void render() {
        rows.removeAllViews();
        ArrayList<Host> sorted = new ArrayList<>(hosts.values()); sorted.sort(Comparator.comparing(h -> h.key));
        for (Host host : sorted) {
            if (!host.known) continue;
            Button button = new Button(activity);
            button.setText(host.name + " · " + (host.busy ? "Busy" : "Request connection") + "\n" + host.ip);
            button.setEnabled(!host.busy && pending == null);
            button.setOnClickListener(v -> {
                pending = new Attempt(host);
                status.setText("Requesting access to " + host.name + "…"); render();
            });
            rows.addView(button);
        }
        cancel.setVisibility(pending == null ? android.view.View.GONE : android.view.View.VISIBLE);
    }
    private void tick(int run) {
        if (generation != run) return;
        Attempt attempt = pending;
        try {
            if (attempt != null) {
                if (System.nanoTime() - attempt.started > 55_000_000_000L) {
                    finish(run, attempt, "Request expired. Select the PC to try again."); return;
                }
                Reply reply = request(attempt.host, "/v3/pair", body(attempt), true);
                if (reply.code == 202) {
                    String code = new JSONObject(reply.body).getString("code");
                    if (!code.matches("[0-9]{6}")) throw new IOException();
                    activity.runOnUiThread(() -> { if (generation == run && pending == attempt) status.setText("Approve on your PC only if this number matches:\n" + code + "\nWaiting for approval…"); });
                } else if (reply.code == 200) {
                    String token = new JSONObject(reply.body).getString("token");
                    if (!token.matches("[0-9a-f]{32}")) throw new IOException();
                    activity.runOnUiThread(() -> {
                        if (generation == run && pending == attempt) { pending = null; connected.connect(attempt.host.ip, attempt.host.port, token, client); }
                    });
                } else {
                    finish(run, attempt, reply.code == 403 ? "Connection denied on the PC." : reply.code == 409 ? "That PC is busy. Try again when it is available." : reply.code == 429 ? "Please wait ten seconds before requesting again." : "Could not request access. Check PC sharing and try again.");
                }
            } else {
                Host next = null;
                for (Host host : hosts.values()) if (System.nanoTime() - host.checked > 5_000_000_000L && (next == null || host.checked < next.checked)) next = host;
                if (next == null) return;
                Host host = next; host.checked = System.nanoTime();
                try {
                    Reply reply = request(host, "/v3/info", null, true);
                    JSONObject info = new JSONObject(reply.body);
                    String name = info.getString("name");
                    if (reply.code != 200 || info.getInt("protocol") != 3 || !name.matches("[A-Za-z0-9 _().-]{1,64}")) throw new IOException();
                    host.name = name; host.busy = info.getBoolean("busy"); host.known = true;
                } catch (Exception error) { host.known = false; }
                activity.runOnUiThread(() -> {
                    if (generation == run && pending == null) {
                        status.setText("Choose a PC below, then approve the request on that PC."); render();
                    }
                });
            }
        } catch (Exception error) { if (attempt != null) finish(run, attempt, "Connection interrupted. Check the same Wi-Fi and request again."); }
    }
    private void finish(int run, Attempt attempt, String text) {
        activity.runOnUiThread(() -> { if (generation == run && pending == attempt) { pending = null; status.setText(text); render(); } });
        cancelRemote(attempt);
    }
    private Reply request(Host host, String path, String body, boolean track) throws Exception {
        HttpURLConnection c = (HttpURLConnection) new URL("http://" + host.ip + ":" + host.port + path).openConnection(Proxy.NO_PROXY);
        if (track) connection = c;
        long deadline = System.nanoTime() + 3_000_000_000L;
        c.setConnectTimeout(1000); c.setReadTimeout(1000); c.setInstanceFollowRedirects(false); c.setUseCaches(false);
        c.setRequestProperty("X-Pluriview-Client", client); c.setRequestProperty("Connection", "close");
        try {
            if (body != null) {
                byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
                c.setRequestMethod("POST"); c.setDoOutput(true); c.setFixedLengthStreamingMode(bytes.length);
                c.setRequestProperty("Content-Type", "application/json");
                try (OutputStream output = c.getOutputStream()) { output.write(bytes); }
            }
            int code = c.getResponseCode();
            if (code != 200 && code != 202) return new Reply(code, "");
            try (InputStream input = c.getInputStream(); ByteArrayOutputStream output = new ByteArrayOutputStream()) {
                byte[] bytes = new byte[512];
                while (true) {
                    long remaining = deadline - System.nanoTime(); if (remaining <= 0) throw new IOException();
                    c.setReadTimeout((int) Math.min(1000, Math.max(1, remaining / 1_000_000)));
                    int count = input.read(bytes); if (count == -1) break;
                    if (output.size() + count > 2048) throw new IOException(); output.write(bytes, 0, count);
                }
                return new Reply(code, new String(output.toByteArray(), StandardCharsets.UTF_8));
            }
        } finally { c.disconnect(); if (track && connection == c) connection = null; }
    }
}
