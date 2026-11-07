package io.pluriview.viewer;

import android.app.Activity;
import android.graphics.Typeface;
import android.os.Build;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.UUID;
import java.util.concurrent.*;
import org.json.JSONObject;

/** Nearby discovery never obtains viewer access until the PC owner approves. */
final class PairingScreen {
    interface Connected { void connect(String ip, int port, String token, String client, String name); }
    /** Android cancels a request shortly before the PC's 60-second expiry. */
    private static final long REQUEST_NANOS = 55_000_000_000L;
    private final Activity activity;
    private final Connected connected;
    private final LinearLayout rows;
    private final TextView empty, message;
    private final LinearLayout tips;
    private final ProgressBar searching;
    private final LinearLayout request;
    private final TextView requestTitle, requestHint, code, waiting;
    private final ProgressBar requestSpinner;
    private final HashMap<String, Card> cards = new HashMap<>();
    private final ConcurrentHashMap<String, Host> hosts = new ConcurrentHashMap<>();
    private final String client = UUID.randomUUID().toString().replace("-", "");
    private final Runnable showTips = () -> { tipsDue = true; render(); };
    private final Runnable countdown = new Runnable() { public void run() { renderRequest(); if (pending != null) rows.postDelayed(this, 1000); } };
    private NearbyDiscovery discovery;
    private ScheduledExecutorService worker;
    private ScheduledFuture<?> timer;
    private volatile HttpURLConnection connection;
    private volatile Attempt pending;
    private volatile int generation;
    private boolean running, tipsDue, discoveryFailed;

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
        volatile String code;
        Attempt(Host host) { this.host = host; }
    }
    private static final class Reply {
        final int code; final String body;
        Reply(int code, String body) { this.code = code; this.body = body; }
    }
    private static final class Card {
        final LinearLayout view; final TextView name, detail;
        Card(LinearLayout view, TextView name, TextView detail) { this.view = view; this.name = name; this.detail = detail; }
    }

    PairingScreen(Activity activity, LinearLayout root, Connected connected) {
        this.activity = activity; this.connected = connected;
        LinearLayout heading = new LinearLayout(activity); heading.setGravity(Gravity.CENTER_VERTICAL);
        heading.addView(Ui.label(activity, "Nearby PCs"));
        searching = Ui.spinner(activity, 16);
        ((LinearLayout.LayoutParams) searching.getLayoutParams()).leftMargin = Ui.dp(activity, 10);
        heading.addView(searching);
        heading.addView(new View(activity), new LinearLayout.LayoutParams(0, 1, 1));
        Button refresh = Ui.button(activity, "Refresh", Ui.Kind.QUIET);
        refresh.setOnClickListener(v -> { message(null, false); stop(); start(); });
        heading.addView(refresh, new LinearLayout.LayoutParams(-2, Ui.dp(activity, 40)));
        root.addView(heading, Ui.spaced(activity, 24));

        rows = new LinearLayout(activity); rows.setOrientation(LinearLayout.VERTICAL);
        root.addView(rows, Ui.spaced(activity, 4));
        empty = Ui.text(activity, "Looking for PCs sharing on this Wi-Fi…", 15, Ui.SECONDARY);
        root.addView(empty, Ui.spaced(activity, 8));
        tips = Ui.card(activity, LinearLayout.VERTICAL);
        tips.addView(Ui.text(activity, "Not finding your PC?", 15, Ui.TEXT));
        tips.addView(Ui.text(activity, "• On the PC, choose Tools → Phone Viewer and start sharing.\n"
            + "• Connect the phone and PC to the same private Wi-Fi.\n"
            + "• Some networks block device discovery. Use a pairing link below instead.", 14, Ui.SECONDARY), Ui.spaced(activity, 6));
        root.addView(tips, Ui.spaced(activity, 12));

        request = Ui.card(activity, LinearLayout.VERTICAL);
        request.setBackground(Ui.rounded(Ui.SURFACE, 16, Ui.GOLD, activity));
        request.setGravity(Gravity.CENTER_HORIZONTAL);
        requestTitle = Ui.text(activity, "", 16, Ui.TEXT); requestTitle.setGravity(Gravity.CENTER);
        request.addView(requestTitle);
        requestSpinner = Ui.spinner(activity, 28);
        ((LinearLayout.LayoutParams) requestSpinner.getLayoutParams()).topMargin = Ui.dp(activity, 16);
        request.addView(requestSpinner);
        requestHint = Ui.text(activity, "", 14, Ui.SECONDARY); requestHint.setGravity(Gravity.CENTER);
        request.addView(requestHint, Ui.spaced(activity, 12));
        code = Ui.text(activity, "", 40, Ui.GOLD);
        code.setTypeface(Typeface.MONOSPACE, Typeface.BOLD); code.setLetterSpacing(.12f); code.setGravity(Gravity.CENTER);
        request.addView(code, Ui.spaced(activity, 4));
        waiting = Ui.text(activity, "", 13, Ui.SECONDARY); waiting.setGravity(Gravity.CENTER);
        request.addView(waiting, Ui.spaced(activity, 4));
        Button cancel = Ui.button(activity, "Cancel request", Ui.Kind.SECONDARY);
        cancel.setOnClickListener(v -> cancelRequest());
        request.addView(cancel, Ui.spaced(activity, 16));
        root.addView(request, Ui.spaced(activity, 12));

        message = Ui.text(activity, "", 14, Ui.SECONDARY);
        root.addView(message, Ui.spaced(activity, 12));
        message(null, false);
        render();
    }

    void start() {
        if (running) return;
        running = true; final int run = ++generation;
        hosts.clear(); tipsDue = false; discoveryFailed = false; searching.setVisibility(View.VISIBLE); render();
        rows.postDelayed(showTips, 8000);
        worker = Executors.newSingleThreadScheduledExecutor();
        discovery = new NearbyDiscovery(activity, new NearbyDiscovery.Listener() {
            public void found(String key, String ip, int port) {
                if (generation == run && hosts.size() < 16) { hosts.put(key, new Host(key, ip, port)); }
            }
            public void lost(String key) { if (generation == run) { hosts.remove(key); render(); } }
            public void unavailable() {
                if (generation != run) return;
                message("Device discovery isn't available on this network. Use a pairing link below, or tap Refresh.", true);
                tipsDue = true; discoveryFailed = true; searching.setVisibility(View.GONE); render();
            }
        });
        timer = worker.scheduleWithFixedDelay(() -> tick(run), 0, 750, TimeUnit.MILLISECONDS);
    }
    void stop() {
        if (!running) return;
        running = false; generation++;
        rows.removeCallbacks(showTips); rows.removeCallbacks(countdown);
        searching.setVisibility(View.GONE);
        if (discovery != null) discovery.close();
        if (timer != null) timer.cancel(false);
        HttpURLConnection active = connection; if (active != null) active.disconnect();
        Attempt previous = pending; pending = null;
        if (previous != null) worker.execute(() -> cancelRemote(previous));
        worker.shutdown();
        render();
    }
    private void choose(Host host) {
        if (pending != null || host.busy) return;
        pending = new Attempt(host);
        message(null, false); render();
        rows.removeCallbacks(countdown); rows.post(countdown);
    }
    private void cancelRequest() {
        Attempt previous = pending; pending = null;
        if (previous != null) worker.execute(() -> cancelRemote(previous));
        message("Request cancelled. You can choose a PC again.", false); render();
    }
    private void cancelRemote(Attempt attempt) { try { request(attempt.host, "/v3/pair/cancel", body(attempt), false); } catch (Exception ignored) {} }
    private String body(Attempt attempt) throws Exception {
        String name = Build.MODEL.replaceAll("[^A-Za-z0-9 _().-]", "").trim();
        if (name.isEmpty()) name = "Android phone";
        return new JSONObject().put("request", attempt.request).put("name", name.substring(0, Math.min(64, name.length()))).toString();
    }
    private void message(String text, boolean error) {
        message.setText(text == null ? "" : text);
        message.setTextColor(error ? Ui.DANGER : Ui.SECONDARY);
        message.setVisibility(text == null ? View.GONE : View.VISIBLE);
    }

    private Card card(Host host) {
        Card existing = cards.get(host.key);
        if (existing != null) return existing;
        LinearLayout view = Ui.card(activity, LinearLayout.HORIZONTAL);
        view.setBackground(Ui.pressable(activity, Ui.SURFACE, 16, Ui.BORDER));
        ImageView badge = new ImageView(activity);
        badge.setImageResource(R.drawable.ic_monitor);
        badge.setImageTintList(android.content.res.ColorStateList.valueOf(Ui.GOLD));
        badge.setBackground(Ui.rounded(Ui.GOLD_DARK, 12, 0, activity));
        int pad = Ui.dp(activity, 10);
        badge.setPadding(pad, pad, pad, pad);
        view.addView(badge, new LinearLayout.LayoutParams(Ui.dp(activity, 44), Ui.dp(activity, 44)));
        LinearLayout labels = new LinearLayout(activity); labels.setOrientation(LinearLayout.VERTICAL);
        TextView name = Ui.text(activity, "", 16, Ui.TEXT);
        TextView detail = Ui.text(activity, "", 13, Ui.SECONDARY);
        labels.addView(name); labels.addView(detail);
        LinearLayout.LayoutParams fill = new LinearLayout.LayoutParams(0, -2, 1);
        fill.leftMargin = fill.rightMargin = Ui.dp(activity, 14);
        view.addView(labels, fill);
        ImageView chevron = new ImageView(activity);
        chevron.setImageResource(R.drawable.ic_chevron);
        chevron.setImageTintList(android.content.res.ColorStateList.valueOf(Ui.SECONDARY));
        view.addView(chevron, new LinearLayout.LayoutParams(Ui.dp(activity, 20), Ui.dp(activity, 20)));
        view.setOnClickListener(v -> choose(host));
        Card card = new Card(view, name, detail);
        cards.put(host.key, card);
        return card;
    }

    /** Update host cards in place; views are only reordered when the set of PCs changes. */
    private void render() {
        ArrayList<Host> known = new ArrayList<>();
        for (Host host : hosts.values()) if (host.known) known.add(host);
        known.sort(Comparator.comparing(h -> h.key));
        boolean reorder = known.size() != rows.getChildCount();
        for (int i = 0; i < known.size(); i++) {
            Host host = known.get(i);
            Card card = card(host);
            card.name.setText(host.name);
            card.detail.setText(host.ip + " · " + (host.busy ? "Busy: another phone is viewing" : "Available"));
            card.view.setContentDescription(host.name + ", " + (host.busy ? "busy" : "request connection"));
            Ui.enable(card.view, !host.busy && pending == null);
            if (!reorder && rows.getChildAt(i) != card.view) reorder = true;
        }
        if (reorder) {
            java.util.HashSet<String> keep = new java.util.HashSet<>();
            rows.removeAllViews();
            for (Host host : known) { keep.add(host.key); rows.addView(cards.get(host.key).view, Ui.spaced(activity, 8)); }
            cards.keySet().retainAll(keep);
        }
        boolean none = known.isEmpty() && pending == null;
        empty.setText(discoveryFailed ? "No PCs found." : running ? "Looking for PCs sharing on this Wi-Fi…" : "Search paused.");
        empty.setVisibility(none ? View.VISIBLE : View.GONE);
        tips.setVisibility(none && tipsDue ? View.VISIBLE : View.GONE);
        renderRequest();
    }

    private void renderRequest() {
        Attempt attempt = pending;
        request.setVisibility(attempt == null ? View.GONE : View.VISIBLE);
        if (attempt == null) return;
        String number = attempt.code;
        requestTitle.setText("Requesting access to " + attempt.host.name);
        requestSpinner.setVisibility(number == null ? View.VISIBLE : View.GONE);
        requestHint.setText(number == null ? "Sending the request…" : "Approve on the PC only if it shows this number");
        code.setVisibility(number == null ? View.GONE : View.VISIBLE);
        if (number != null) {
            code.setText(number.substring(0, 3) + " " + number.substring(3));
            code.setContentDescription(number.replace("", " ").trim());
        }
        long left = Math.max(0, (REQUEST_NANOS - (System.nanoTime() - attempt.started)) / 1_000_000_000L);
        waiting.setText(number == null ? "" : "Waiting for approval · " + left + " s left");
        waiting.setVisibility(number == null ? View.GONE : View.VISIBLE);
    }

    private void ui(int run, Runnable action) {
        activity.runOnUiThread(() -> { if (generation == run) action.run(); });
    }

    private void tick(int run) {
        if (generation != run) return;
        Attempt attempt = pending;
        try {
            if (attempt != null) {
                if (System.nanoTime() - attempt.started > REQUEST_NANOS) {
                    finish(run, attempt, "The request expired. Choose the PC to try again."); return;
                }
                Reply reply = request(attempt.host, "/v3/pair", body(attempt), true);
                if (reply.code == 202) {
                    String number = new JSONObject(reply.body).getString("code");
                    if (!number.matches("[0-9]{6}")) throw new IOException();
                    attempt.code = number;
                    ui(run, () -> { if (pending == attempt) renderRequest(); });
                } else if (reply.code == 200) {
                    String token = new JSONObject(reply.body).getString("token");
                    if (!token.matches("[0-9a-f]{32}")) throw new IOException();
                    ui(run, () -> {
                        if (pending == attempt) { pending = null; connected.connect(attempt.host.ip, attempt.host.port, token, client, attempt.host.name); }
                    });
                } else {
                    finish(run, attempt, reply.code == 403 ? "The PC denied the connection." : reply.code == 409 ? "That PC is busy with another phone. Try again when it's free." : reply.code == 429 ? "Please wait ten seconds before requesting again." : "Couldn't request access. Check that the PC is still sharing, then try again.");
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
                ui(run, () -> { if (pending == null) render(); });
            }
        } catch (Exception error) { if (attempt != null) finish(run, attempt, "The connection was interrupted. Check that both devices are on the same Wi-Fi, then try again."); }
    }
    private void finish(int run, Attempt attempt, String text) {
        ui(run, () -> { if (pending == attempt) { pending = null; message(text, true); render(); } });
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
