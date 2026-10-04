package io.pluriview.viewer;

import android.app.Activity;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.content.Intent;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.RectF;
import android.graphics.SurfaceTexture;
import android.graphics.Matrix;
import android.net.Uri;
import android.os.Bundle;
import android.os.Build;
import android.view.MotionEvent;
import android.view.View;
import android.view.TextureView;
import android.view.Surface;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.view.ViewConfiguration;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.ScrollView;
import java.io.DataInputStream;
import java.io.BufferedInputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.UUID;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

public final class ViewerActivity extends Activity {
    private static final int GOLD = Color.rgb(226, 186, 105);
    private Session session;
    private PairingScreen nearby;
    private final DecoderSlot decoderSlot = new DecoderSlot();
    private final DecoderSlot audioNetworkSlot = new DecoderSlot(), audioCodecSlot = new DecoderSlot();
    private TextView status;
    private CanvasView canvas;
    private EditText pairing;
    private boolean foreground;
    private boolean fullscreen;
    private boolean fill;
    private LinearLayout viewerRoot, viewerBar, overlay;
    private TextView viewerHint;
    private TextView viewerStats;
    private Button fitButton, overlayFit;
    private Button audioButton, overlayAudio;
    private TextView audioStatus, overlayAudioStatus;
    private final Runnable hideControls = () -> { if (fullscreen && overlay != null) overlay.setVisibility(View.GONE); };

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        setVolumeControlStream(AudioManager.STREAM_MUSIC);
        fill = getPreferences(MODE_PRIVATE).getBoolean("fill", true);
        getWindow().setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE);
        showPairing(null);
        acceptIntent(getIntent());
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        acceptIntent(intent);
    }

    private void acceptIntent(Intent intent) {
        if (intent != null && intent.getData() != null) {
            // External QR links prefill pairing; they never start a connection silently.
            showPairing(intent.getData().toString());
            intent.setData(null);
        }
    }

    private int dp(int value) { return (int) (value * getResources().getDisplayMetrics().density + .5f); }

    private TextView text(String value, int size) {
        TextView view = new TextView(this);
        view.setText(value); view.setTextColor(Color.WHITE); view.setTextSize(size);
        view.setPadding(0, dp(8), 0, dp(8));
        return view;
    }

    private LinearLayout column() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(Color.rgb(19, 19, 22));
        root.setPadding(dp(20), dp(24), dp(20), dp(16));
        root.setOnApplyWindowInsetsListener((v, insets) -> {
            if (fullscreen) { v.setPadding(0, 0, 0, 0); return insets; }
            v.setPadding(dp(20) + insets.getSystemWindowInsetLeft(), dp(16) + insets.getSystemWindowInsetTop(),
                dp(20) + insets.getSystemWindowInsetRight(), dp(12) + insets.getSystemWindowInsetBottom());
            return insets;
        });
        return root;
    }

    private void showPairing(String invite) {
        if (nearby != null) nearby.stop();
        if (viewerRoot != null) viewerRoot.removeCallbacks(hideControls);
        fullscreen = false;
        stopSession();
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        LinearLayout root = column();
        TextView title = text("Pluriview Viewer", 28); title.setTextColor(GOLD); root.addView(title);
        root.addView(text("Your canvas, on your phone", 18));
        root.addView(text("On your PC, open View → Phone Viewer and start sharing. Choose it below and approve the request on the PC.", 16));
        nearby = new PairingScreen(this, root, this::openViewer);
        root.addView(text("Or use a QR / pairing link", 16));
        pairing = new EditText(this);
        pairing.setHint("Pairing link"); pairing.setTextColor(Color.WHITE); pairing.setHintTextColor(Color.GRAY);
        pairing.setSingleLine(true);
        pairing.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_VARIATION_PASSWORD);
        pairing.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        if (invite != null) pairing.setText(invite);
        root.addView(pairing);
        Button connect = new Button(this); connect.setText("Connect to PC"); root.addView(connect);
        status = text("Use the same trusted private Wi-Fi as your PC. Streaming is unencrypted. Optional tile audio starts off.", 14);
        root.addView(status);
        connect.setOnClickListener(v -> connect(pairing.getText().toString().trim()));
        ScrollView scroll = new ScrollView(this); scroll.setFillViewport(true); scroll.addView(root);
        setContentView(scroll);
        applySystemBars();
        if (foreground) nearby.start();
    }

    private void connect(String invite) {
        try {
            Uri uri = Uri.parse(invite);
            String ip = uri.getQueryParameter("host");
            String token = uri.getQueryParameter("token");
            int port = Integer.parseInt(uri.getQueryParameter("port"));
            if (!"pluriview".equals(uri.getScheme()) || !"viewer".equals(uri.getHost())
                    || !privateIpv4(ip) || port < 1 || port > 65535 || token == null || !token.matches("[0-9a-f]{32}"))
                throw new IllegalArgumentException();
            openViewer(ip, port, token, UUID.randomUUID().toString().replace("-", ""));
        } catch (Exception e) {
            status.setText("That pairing link is not valid. Copy a fresh link from Phone Viewer on the PC.");
        }
    }

    private void openViewer(String ip, int port, String token, String client) {
        if (nearby != null) nearby.stop();
        stopSession();
        Session next = new Session(ip, port, token, client);
        session = next;
        showViewer(next);
        if (foreground) next.start();
    }

    static boolean privateIpv4(String ip) {
        if (ip == null || !ip.matches("[0-9]{1,3}(\\.[0-9]{1,3}){3}")) return false;
        String[] parts = ip.split("\\.");
        int[] octets = new int[4];
        for (int i = 0; i < 4; i++) {
            if (parts[i].length() > 1 && parts[i].startsWith("0")) return false;
            octets[i] = Integer.parseInt(parts[i]);
            if (octets[i] > 255) return false;
        }
        return octets[0] == 10 || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
            || (octets[0] == 192 && octets[1] == 168) || octets[0] == 127;
    }

    private void showViewer(Session next) {
        FrameLayout surface = new FrameLayout(this);
        LinearLayout root = column();
        viewerRoot = root;
        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.VERTICAL);
        viewerBar = bar;
        status = text("Connecting…", 14);
        bar.addView(status, new LinearLayout.LayoutParams(-1, -2));
        LinearLayout actions = new LinearLayout(this);
        bar.addView(actions, new LinearLayout.LayoutParams(-1, -2));
        Button disconnect = new Button(this); disconnect.setText("Disconnect");
        disconnect.setOnClickListener(v -> showPairing(null)); actions.addView(disconnect, new LinearLayout.LayoutParams(0, -2, 1));
        Button expand = new Button(this); expand.setText("Full screen"); actions.addView(expand, new LinearLayout.LayoutParams(0, -2, 1));
        fitButton = new Button(this); actions.addView(fitButton, new LinearLayout.LayoutParams(0, -2, 1));
        fitButton.setOnClickListener(v -> toggleFit());
        LinearLayout sound = new LinearLayout(this); bar.addView(sound);
        audioButton = new Button(this); sound.addView(audioButton);
        audioButton.setOnClickListener(v -> next.toggleAudio());
        audioStatus = text("Audio: off", 12); sound.addView(audioStatus, new LinearLayout.LayoutParams(0, -2, 1));
        root.addView(bar);
        canvas = new CanvasView(next);
        root.addView(canvas, new LinearLayout.LayoutParams(-1, 0, 1));
        TextView hint = text("Drag to pan · Pinch to zoom · Double-tap a tile to focus", 12);
        viewerHint = hint;
        root.addView(hint);
        surface.addView(root, new FrameLayout.LayoutParams(-1, -1));
        overlay = new LinearLayout(this);
        overlay.setOrientation(LinearLayout.VERTICAL);
        overlay.setBackgroundColor(0xCC131316);
        viewerStats = text("Connecting…", 12); overlay.addView(viewerStats);
        LinearLayout overlayActions = new LinearLayout(this); overlay.addView(overlayActions);
        Button restore = new Button(this); restore.setText("Exit full screen"); overlayActions.addView(restore);
        overlayFit = new Button(this); overlayActions.addView(overlayFit); overlayFit.setOnClickListener(v -> toggleFit());
        LinearLayout overlaySound = new LinearLayout(this); overlay.addView(overlaySound);
        overlayAudio = new Button(this); overlaySound.addView(overlayAudio);
        overlayAudio.setOnClickListener(v -> next.toggleAudio());
        overlayAudioStatus = text("Audio: off", 12); overlaySound.addView(overlayAudioStatus, new LinearLayout.LayoutParams(0, -2, 1));
        FrameLayout.LayoutParams corner = new FrameLayout.LayoutParams(-2, -2, android.view.Gravity.TOP | android.view.Gravity.RIGHT);
        corner.setMargins(dp(8), dp(8), dp(8), dp(8));
        surface.addView(overlay, corner);
        surface.setOnApplyWindowInsetsListener((v, insets) -> {
            int left = insets.getSystemWindowInsetLeft(), top = insets.getSystemWindowInsetTop(), right = insets.getSystemWindowInsetRight();
            if (Build.VERSION.SDK_INT >= 28 && insets.getDisplayCutout() != null) {
                left = Math.max(left, insets.getDisplayCutout().getSafeInsetLeft());
                top = Math.max(top, insets.getDisplayCutout().getSafeInsetTop());
                right = Math.max(right, insets.getDisplayCutout().getSafeInsetRight());
            }
            FrameLayout.LayoutParams safe = (FrameLayout.LayoutParams) overlay.getLayoutParams();
            safe.setMargins(dp(8) + left, dp(8) + top, dp(8) + right, dp(8)); overlay.setLayoutParams(safe);
            return insets;
        });
        expand.setOnClickListener(v -> setFullscreen(true)); restore.setOnClickListener(v -> setFullscreen(false));
        updateFitLabels();
        next.updateAudioLabels();
        setContentView(surface);
        setFullscreen(true);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
    }

    private void updateFitLabels() {
        if (fitButton != null) fitButton.setText(fill ? "Fit whole canvas" : "Fill screen");
        if (overlayFit != null) overlayFit.setText(fill ? "Fit whole canvas" : "Fill screen");
    }
    private void toggleFit() {
        fill = !fill; getPreferences(MODE_PRIVATE).edit().putBoolean("fill", fill).apply();
        updateFitLabels(); canvas.resetGesture(); canvas.layoutVideo(); canvas.invalidate(); revealControls();
    }
    private void setFullscreen(boolean value) {
        fullscreen = value;
        viewerBar.setVisibility(value ? View.GONE : View.VISIBLE);
        viewerHint.setVisibility(value ? View.GONE : View.VISIBLE);
        overlay.setVisibility(value ? View.VISIBLE : View.GONE);
        if (value) viewerRoot.setPadding(0, 0, 0, 0);
        applySystemBars(); viewerRoot.requestApplyInsets(); canvas.resetGesture();
        if (value) revealControls();
    }
    private void revealControls() {
        if (!fullscreen || overlay == null) return;
        overlay.setVisibility(View.VISIBLE); viewerRoot.removeCallbacks(hideControls);
        viewerRoot.postDelayed(hideControls, 3500);
    }
    private void applySystemBars() {
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams attributes = getWindow().getAttributes();
            attributes.layoutInDisplayCutoutMode = fullscreen
                ? (Build.VERSION.SDK_INT >= 30 ? WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES)
                : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_DEFAULT;
            getWindow().setAttributes(attributes);
        }
        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(!fullscreen);
            // Window.getInsetsController() dereferences PhoneWindow's decor even
            // before it exists. getDecorView() initializes it on a cold launch.
            WindowInsetsController controller = getWindow().getDecorView().getWindowInsetsController();
            if (controller != null) {
                controller.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                if (fullscreen) controller.hide(WindowInsets.Type.systemBars()); else controller.show(WindowInsets.Type.systemBars());
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(fullscreen
                ? View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                : View.SYSTEM_UI_FLAG_VISIBLE);
        }
    }
    @Override public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused); if (focused) applySystemBars();
    }

    @Override protected void onStart() {
        super.onStart(); foreground = true;
        if (session != null) session.start();
        else if (nearby != null) nearby.start();
    }

    @Override protected void onStop() {
        foreground = false;
        if (session != null) session.pause();
        if (nearby != null) nearby.stop();
        super.onStop();
    }

    @Override public void onBackPressed() {
        if (session != null && fullscreen) setFullscreen(false);
        else if (session != null) showPairing(null); else super.onBackPressed();
    }

    @Override protected void onDestroy() { if (nearby != null) nearby.stop(); stopSession(); super.onDestroy(); }
    private void stopSession() { if (session != null) session.pause(); session = null; }

    private final class Session {
        final String base, token, client;
        final CameraTransform camera = new CameraTransform();
        final AtomicReference<float[]> pendingFocus = new AtomicReference<>();
        final AtomicBoolean running = new AtomicBoolean(false);
        volatile boolean ready;
        volatile float width, height;
        volatile int generation;
        ScheduledExecutorService workers;
        volatile VideoConnection videoConnection;
        volatile ControlConnection controlConnection;
        boolean legacyControl;
        long nextControlConnect;
        final AtomicBoolean videoUiPending = new AtomicBoolean(false);
        volatile VideoPacket latestVideo;
        volatile int latestVideoRevision;
        final FrameRate receivedRate = new FrameRate(), shownRate = new FrameRate();
        Runnable statsTask;
        volatile int decoderRevision;
        final RecoveryWindow recovery = new RecoveryWindow();
        volatile int reconnects;
        volatile String lastRecovery = "";
        boolean audioEnabled;
        AudioStream audioStream;
        AudioFocusRequest audioFocus;
        int audioRevision;
        int audioFocusRevision;
        String audioText = "Audio: off";

        void updateAudioLabels() {
            if (session != this) return;
            if (audioButton != null) audioButton.setText(audioEnabled ? "Audio on" : "Audio off");
            if (overlayAudio != null) overlayAudio.setText(audioEnabled ? "Audio on" : "Audio off");
            if (audioStatus != null) audioStatus.setText(audioText);
            if (overlayAudioStatus != null) overlayAudioStatus.setText(audioText);
        }

        void toggleAudio() {
            audioEnabled = !audioEnabled;
            audioText = audioEnabled ? "Audio: waiting for canvas" : "Audio: off";
            updateAudio(); updateAudioLabels(); revealControls();
        }

        void stopAudio() {
            audioRevision++;
            audioFocusRevision++;
            if (audioStream != null) { audioStream.close(); audioStream = null; }
            if (audioFocus != null) {
                ((AudioManager) getSystemService(AUDIO_SERVICE)).abandonAudioFocusRequest(audioFocus);
                audioFocus = null;
            }
        }

        void updateAudio() {
            if (!audioEnabled || !running.get() || !foreground || !ready) {
                if (audioStream != null || audioFocus != null) stopAudio();
                if (audioEnabled) audioText = "Audio: waiting for canvas";
                updateAudioLabels(); return;
            }
            if (audioStream != null) return;
            if (!audioNetworkSlot.available() || !audioCodecSlot.available()) {
                audioText = "Audio: waiting for previous playback to stop"; updateAudioLabels(); return;
            }
            if (audioFocus == null) {
                final int focusRevision = ++audioFocusRevision;
                AudioManager manager = (AudioManager) getSystemService(AUDIO_SERVICE);
                audioFocus = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                    .setAudioAttributes(new AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
                    .setWillPauseWhenDucked(true)
                    .setOnAudioFocusChangeListener(change -> {
                        if (change < 0 && session == this && audioFocusRevision == focusRevision) {
                            audioEnabled = false; stopAudio(); audioText = "Audio off: another app needs sound"; updateAudioLabels();
                        }
                    }).build();
                if (manager.requestAudioFocus(audioFocus) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
                    audioEnabled = false; stopAudio(); audioText = "Audio off: sound is in use"; updateAudioLabels(); return;
                }
            }
            final int revision = ++audioRevision;
            try {
                audioStream = new AudioStream(base, token, client, audioNetworkSlot, audioCodecSlot, value -> runOnUiThread(() -> {
                    if (session == this && audioRevision == revision && audioEnabled && running.get() && !value.equals(audioText)) {
                        audioText = value; updateAudioLabels();
                    }
                }));
            } catch (java.io.IOException error) { stopAudio(); audioText = "Audio: waiting for previous playback to stop"; }
            updateAudioLabels();
        }

        Session(String ip, int port, String token, String client) { base = "http://" + ip + ":" + port; this.token = token; this.client = client; }

        void start() {
            if (!running.compareAndSet(false, true)) return;
            final int run = ++generation;
            ready = false; camera.clear(); latestVideo = null;
            recovery.reset(); reconnects = 0; lastRecovery = "";
            receivedRate.clear(); shownRate.clear();
            message(run, "Connecting to 720p video…", true);
            workers = Executors.newScheduledThreadPool(2);
            workers.execute(() -> streamVideo(run));
            workers.scheduleWithFixedDelay(() -> sendCamera(run), 0, 8, TimeUnit.MILLISECONDS);
            statsTask = new Runnable() { public void run() {
                if (!current(run) || session != Session.this) return;
                updateAudio(); updateStats(); canvas.postDelayed(this, 500);
            }};
            canvas.post(statsTask);
        }

        void pause() {
            running.set(false); generation++; ready = false; camera.clear(); pendingFocus.set(null);
            stopAudio();
            recovery.reset();
            latestVideo = null;
            if (canvas != null && canvas.owner == this) { canvas.video.setAlpha(0); canvas.resetGesture(); canvas.invalidate(); }
            decoderRevision++;
            if (statsTask != null) canvas.removeCallbacks(statsTask);
            VideoConnection connection = videoConnection;
            if (connection != null) connection.close();
            ControlConnection control = controlConnection;
            if (control != null) control.close(); controlConnection = null;
            if (workers != null) workers.shutdownNow();
        }

        boolean current(int run) { return running.get() && generation == run; }

        HttpURLConnection request(String path, String method) throws Exception {
            HttpURLConnection c = (HttpURLConnection) new URL(base + path).openConnection(java.net.Proxy.NO_PROXY);
            c.setConnectTimeout(1500); c.setReadTimeout(2000); c.setInstanceFollowRedirects(false);
            c.setUseCaches(false); c.setRequestMethod(method);
            c.setRequestProperty("Authorization", Auth.header(token, method, path, client));
            c.setRequestProperty("X-Pluriview-Client", client);
            c.setRequestProperty("Connection", "close");
            return c;
        }

        void message(int run, String message, boolean clear) {
            if (!current(run)) return;
            if (clear) { recovery.reset(); ready = false; camera.clear(); pendingFocus.set(null); latestVideo = null; }
            runOnUiThread(() -> {
                if (!current(run) || session != this) return;
                status.setText(message);
                if (viewerStats != null) viewerStats.setText(message);
                if (clear) { canvas.video.setAlpha(0); canvas.resetGesture(); canvas.invalidate(); }
                if (clear) updateAudio();
            });
        }

        void recoverVideo(int run, Exception error) {
            if (!current(run)) return;
            // Invalidate callbacks from the outgoing decoder before retaining its
            // last texture. A reconnect must not keep accepting stale gestures.
            decoderRevision++;
            reconnects++;
            lastRecovery = recoveryReason(error);
            holdVideo(run, "Reconnecting video · " + lastRecovery + " · " + reconnects);
        }

        void holdVideo(int run, String text) {
            if (!current(run)) return;
            ready = false; camera.clear(); pendingFocus.set(null); latestVideo = null;
            message(run, text, false);
            if (recovery.begin(System.nanoTime())) {
                runOnUiThread(() -> {
                    if (!current(run) || session != this) return;
                    canvas.resetGesture();
                    canvas.postDelayed(() -> {
                        if (current(run) && session == this && recovery.expired(System.nanoTime())) {
                            canvas.video.setAlpha(0); canvas.invalidate();
                        }
                    }, 1000);
                });
            }
        }

        String recoveryReason(Exception error) {
            // Only fixed labels enter the UI; never echo URLs, credentials or
            // arbitrary server/driver error text.
            if (error instanceof java.net.SocketTimeoutException)
                return "Video response timeout".equals(error.getMessage()) ? "Connection response timeout" : "Video data timeout";
            if (error instanceof java.io.EOFException) return "PC stream ended";
            String detail = error.getMessage();
            if ("Video decoder backlog".equals(detail)) return "Decoder queue full";
            if ("Video decoder stalled".equals(detail)) return "Decoder stalled";
            if ("Restart video decoder".equals(detail)) return "Decoder reset";
            if ("Missing video frames".equals(detail)) return "Missing video frames";
            if ("Video driver did not stop".equals(detail)) return "Decoder stopping";
            if (error instanceof java.net.SocketException) return "Connection interrupted";
            return "Video stream interrupted";
        }

        void videoReady(int run, int revision, VideoPacket packet) {
            if (!current(run) || decoderRevision != revision) return;
            latestVideo = packet;
            latestVideoRevision = revision;
            // Keep one UI update, even if layout/rotation briefly stalls the
            // main thread. The network worker must not retain a frame backlog.
            if (!videoUiPending.compareAndSet(false, true)) return;
            runOnUiThread(() -> {
                videoUiPending.set(false);
                if (!current(run) || session != this) return;
                VideoPacket newest = latestVideo;
                if (newest == null || latestVideoRevision != decoderRevision) return;
                recovery.reset();
                if (width != newest.viewportWidth || height != newest.viewportHeight) canvas.resetGesture();
                width = newest.viewportWidth; height = newest.viewportHeight; ready = true;
                canvas.showVideo(newest.width, newest.height);
            });
        }

        void updateStats() {
            VideoPacket newest = latestVideo;
            if (!ready || newest == null) return;
            long now = System.nanoTime();
            String rate = "Received " + receivedRate.fps(now) + " · Shown " + shownRate.fps(now) + " FPS";
            if (reconnects > 0) rate += " · Reconnects " + reconnects + " · " + lastRecovery;
            status.setText("Live · " + newest.width + "×" + newest.height + " · " + rate);
            viewerStats.setText(rate);
        }

        void streamVideo(int run) {
            while (current(run)) {
                if (!decoderSlot.available()) {
                    message(run, "Waiting for the previous video decoder to stop…", true);
                    if (!delay(run, 250)) return;
                    continue;
                }
                SurfaceTexture texture = canvas.texture;
                if (texture == null) { if (!delay(run, 50)) return; continue; }
                VideoConnection connection = null;
                VideoDecoder decoder = null;
                Surface surface = null;
                int retryMillis = 150;
                try {
                    connection = new VideoConnection(base, token, client, () -> holdVideo(run, "Waiting for video data…"));
                    if (!current(run)) return;
                    videoConnection = connection;
                    int code = connection.status;
                    if (code == 404) { message(run, "Update Pluriview on the PC to use video streaming.", true); return; }
                    if (code == 401) { message(run, "Pairing expired. Disconnect and scan a fresh PC code.", true); if (!delay(run, 1000)) return; continue; }
                    if (code == 409) { message(run, "Waiting for the previous viewer connection to close…", false); if (!delay(run, 500)) return; continue; }
                    if (code != 200) throw new java.io.IOException();
                    surface = new Surface(texture);
                    long sequence = 0;
                    {
                        while (current(run) && canvas.texture == texture) {
                            VideoPacket packet = connection.next();
                            if (!current(run)) break;
                            if (packet.kind != 1) {
                                decoderRevision++;
                                if (decoder != null) { closeDecoder(decoder); decoder = null; }
                                sequence = 0;
                                if (packet.kind == 3) { message(run, "PC video encoder unavailable. Restart Phone Viewer sharing on the PC.", true); retryMillis = 2000; break; }
                                message(run, "Waiting for canvas — close PC dialogs and exit browser interaction.", true);
                                continue;
                            }
                            if (sequence != 0 && packet.sequence != sequence + 1) throw new java.io.IOException("Missing video frames");
                            sequence = packet.sequence;
                            receivedRate.record(System.nanoTime());
                            if (decoder == null || decoder.width != packet.width || decoder.height != packet.height) {
                                if (decoder != null) closeDecoder(decoder);
                                final int revision = ++decoderRevision;
                                decoder = new VideoDecoder(surface, packet, shown -> {
                                    if (current(run) && decoderRevision == revision && canvas.texture == texture) videoReady(run, revision, shown);
                                }, decoderSlot);
                            }
                            decoder.queue(packet);
                        }
                    }
                } catch (VideoDecoder.Unavailable error) {
                    retryMillis = 2000;
                    message(run, "Video decoder unavailable. Retrying video…", true);
                } catch (Exception error) {
                    recoverVideo(run, error);
                } finally {
                    if (current(run)) decoderRevision++;
                    if (decoder != null) {
                        try { decoder.close(); } catch (RuntimeException ignored) {}
                    }
                    if (surface != null) surface.release();
                    if (connection != null) connection.close();
                    if (videoConnection == connection) videoConnection = null;
                }
                if (!delay(run, retryMillis)) return;
            }
        }

        boolean delay(int run, int millis) {
            try { Thread.sleep(millis); return current(run); }
            catch (InterruptedException e) { Thread.currentThread().interrupt(); return false; }
        }

        void closeDecoder(VideoDecoder decoder) throws java.io.IOException {
            decoder.close();
            // A stuck vendor driver must not accumulate another worker on every
            // resize or pause. Ordinary stream stalls still reconnect as H.264.
            if (!decoder.terminated()) throw new java.io.IOException("Video driver did not stop");
        }


        void sendCamera(int run) {
            if (!current(run)) return;
            if (!ready) { camera.clear(); pendingFocus.set(null); return; }
            // Leave unsent transforms accumulated during connection backoff.
            if (!legacyControl && controlConnection == null && System.nanoTime() < nextControlConnect) return;
            float[] focus = pendingFocus.getAndSet(null);
            float[] transform = focus == null ? camera.take() : null;
            if (focus == null && transform == null) {
                ControlConnection control = controlConnection;
                if (control != null && System.nanoTime() - control.lastSend > 1_000_000_000L) {
                    try { if (control.send("ping", null) != 200) throw new java.io.IOException(); }
                    catch (Exception ignored) { control.close(); if (controlConnection == control) controlConnection = null; }
                }
                return;
            }
            HttpURLConnection c = null;
            ControlConnection commandControl = controlConnection;
            try {
                String body = focus != null
                    ? "{\"at\":[" + focus[0] + "," + focus[1] + "],\"viewport\":[" + focus[2] + "," + focus[3] + "]}"
                    : "{\"pan\":[" + transform[1] + "," + transform[2] + "],\"scale\":" + transform[0]
                        + ",\"anchor\":[" + transform[3] + "," + transform[4] + "],\"viewport\":[" + width + "," + height + "]}";
                if (!legacyControl) {
                    if (commandControl == null) {
                        try {
                            commandControl = new ControlConnection(base, token, client);
                            if (!current(run)) { commandControl.close(); return; }
                            controlConnection = commandControl;
                        }
                        catch (ControlConnection.Unsupported unsupported) { legacyControl = true; }
                    }
                    if (!legacyControl) {
                        if (!current(run)) { commandControl.close(); if (controlConnection == commandControl) controlConnection = null; return; }
                        if (commandControl.send(focus == null ? "camera" : "focus", body) != 200 && current(run)) camera.clear();
                        return;
                    }
                }
                byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
                c = request(focus == null ? "/v1/camera" : "/v1/focus", "POST"); c.setDoOutput(true);
                c.setRequestProperty("Content-Type", "application/json"); c.setFixedLengthStreamingMode(bytes.length);
                try (java.io.OutputStream output = c.getOutputStream()) { output.write(bytes); }
                int code = c.getResponseCode();
                if (code != 200) camera.clear();
            } catch (Exception e) {
                // Never replay an uncertain gesture: it may already have moved the PC.
                if (commandControl != null) commandControl.close();
                if (controlConnection == commandControl) controlConnection = null;
                if (current(run)) { camera.clear(); nextControlConnect = System.nanoTime() + 250_000_000L; }
            } finally { if (c != null) c.disconnect(); }
        }

    }

    private final class CanvasView extends FrameLayout {
        final Session owner;
        final RectF display = new RectF();
        final TextureView video;
        volatile SurfaceTexture texture;
        int videoWidth, videoHeight;
        float previousX, previousY, previousSpan;
        int pointers;
        boolean gesture;
        boolean moved;
        float downX, downY;
        final DoubleTap taps = new DoubleTap(ViewConfiguration.getDoubleTapTimeout(), ViewConfiguration.getLongPressTimeout(), ViewConfiguration.get(ViewerActivity.this).getScaledDoubleTapSlop());
        final Runnable tapControls = () -> revealControls();

        CanvasView(Session owner) {
            super(ViewerActivity.this); this.owner = owner; setWillNotDraw(false);
            setContentDescription("Live PC canvas. Drag to pan, pinch to zoom, double-tap a tile to toggle focus.");
            video = new TextureView(ViewerActivity.this); video.setAlpha(0);
            addView(video, new FrameLayout.LayoutParams(-1, -1, android.view.Gravity.CENTER));
            video.setSurfaceTextureListener(new TextureView.SurfaceTextureListener() {
                @Override public void onSurfaceTextureAvailable(SurfaceTexture value, int w, int h) { texture = value; }
                @Override public void onSurfaceTextureSizeChanged(SurfaceTexture value, int w, int h) {}
                @Override public void onSurfaceTextureUpdated(SurfaceTexture value) { if (owner.current(owner.generation)) owner.shownRate.record(System.nanoTime()); }
                @Override public boolean onSurfaceTextureDestroyed(SurfaceTexture value) { texture = null; owner.ready = false; resetGesture(); return true; }
            });
        }

        void showVideo(int w, int h) {
            if (videoWidth != w || videoHeight != h || video.getAlpha() == 0) {
                videoWidth = w; videoHeight = h; layoutVideo(); video.setAlpha(1); invalidate();
            }
        }

        void layoutVideo() {
            if (videoWidth == 0 || videoHeight == 0 || getWidth() == 0 || getHeight() == 0) return;
            DisplayMapping mapping = new DisplayMapping(getWidth(), getHeight(), videoWidth, videoHeight, fill);
            display.set(mapping.left, mapping.top, mapping.left + mapping.width, mapping.top + mapping.height);
            Matrix matrix = new Matrix();
            matrix.setScale(mapping.width / getWidth(), mapping.height / getHeight());
            matrix.postTranslate(mapping.left, mapping.top); video.setTransform(matrix);
        }

        @Override protected void onDraw(Canvas c) {
            c.drawColor(Color.BLACK);
            if (video.getAlpha() == 0) display.setEmpty();
        }

        void resetGesture() { gesture = false; pointers = 0; owner.camera.clear(); owner.pendingFocus.set(null); taps.clear(); removeCallbacks(tapControls); }
        @Override protected void onSizeChanged(int w, int h, int oldW, int oldH) { resetGesture(); layoutVideo(); }

        @Override public boolean onInterceptTouchEvent(MotionEvent event) { return true; }

        @Override public boolean onTouchEvent(MotionEvent event) {
            int action = event.getActionMasked();
            if (!owner.ready || display.isEmpty()) {
                if (action == MotionEvent.ACTION_UP) revealControls();
                resetGesture(); return true;
            }
            if (action == MotionEvent.ACTION_DOWN) {
                gesture = display.contains(event.getX(), event.getY()); pointers = 0;
                moved = false; downX = event.getX(); downY = event.getY();
            }
            if (!gesture) {
                taps.clear();
                if (action == MotionEvent.ACTION_UP) revealControls();
                return true;
            }
            if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
                if (action == MotionEvent.ACTION_UP && !moved) {
                    removeCallbacks(tapControls);
                    if (taps.up(event.getDownTime(), event.getEventTime(), event.getX(), event.getY())) {
                        owner.camera.clear();
                        owner.pendingFocus.set(new float[]{(event.getX() - display.left) / display.width(), (event.getY() - display.top) / display.height(), owner.width, owner.height});
                    } else postDelayed(tapControls, ViewConfiguration.getDoubleTapTimeout());
                } else { taps.clear(); removeCallbacks(tapControls); }
                gesture = false; pointers = 0;
                if (action == MotionEvent.ACTION_CANCEL) owner.camera.clear();
                return true;
            }
            // Rebase when fingers enter or leave, preventing a midpoint jump.
            if (action == MotionEvent.ACTION_POINTER_UP) { taps.clear(); pointers = 0; return true; }
            int count = Math.min(2, event.getPointerCount());
            if (count == 2 || Math.hypot(event.getX() - downX, event.getY() - downY) > dp(8)) moved = true;
            if (moved) { taps.clear(); removeCallbacks(tapControls); }
            float x = event.getX(0), y = event.getY(0), span = 0;
            if (count == 2) {
                x = (x + event.getX(1)) / 2; y = (y + event.getY(1)) / 2;
                span = (float) Math.hypot(event.getX(0) - event.getX(1), event.getY(0) - event.getY(1));
            }
            x = (x - display.left) / display.width(); y = (y - display.top) / display.height();
            if (action == MotionEvent.ACTION_MOVE && pointers == count && moved) {
                float factor = count == 2 && previousSpan > dp(8) ? span / previousSpan : 1;
                owner.camera.append(factor, CameraTransform.anchoredTranslation(previousX, x, factor),
                    CameraTransform.anchoredTranslation(previousY, y, factor), previousX, previousY);
            }
            previousX = x; previousY = y; previousSpan = span; pointers = count;
            return true;
        }
    }
}
