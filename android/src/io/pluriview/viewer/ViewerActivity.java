package io.pluriview.viewer;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.content.res.Configuration;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.SurfaceTexture;
import android.graphics.Typeface;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.TextureView;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

public final class ViewerActivity extends Activity {
    private static final String HINT_SHOWN = "gestureHintShown";
    private static final String AUDIO_PLAYING = "Audio: on · stereo";
    /** What the viewer is doing; the status chip and centre panel are drawn from it. */
    enum State { CONNECTING, LIVE, RECOVERING, WAITING, ERROR }

    private Session session;
    private PairingScreen nearby;
    private final DecoderSlot decoderSlot = new DecoderSlot();
    private final DecoderSlot audioNetworkSlot = new DecoderSlot(), audioCodecSlot = new DecoderSlot();
    private boolean foreground;
    private boolean fullscreen;
    private boolean fill;
    private boolean controlsShown, statsOpen;
    private Object backCallback;
    private int density;
    // Pairing screen.
    private EditText pairing;
    private TextView linkStatus;
    private Button connectLink;
    // Viewer screen. Both windowed and fullscreen viewing use these same controls.
    private FrameLayout surface, chrome;
    private CanvasView canvas;
    private LinearLayout topLeft, bottom, statsCard, statePanel, hint;
    private View chipDot;
    private LinearLayout chip;
    private TextView chipText, statsText, audioNote, stateTitle, stateDetail;
    private ProgressBar stateSpinner;
    private Button stateAction;
    private ImageButton fitButton, audioButton, statsButton, fullscreenButton;
    private final Runnable hideControls = () -> { if (fullscreen && !statsOpen) setControls(false); };
    private final Runnable hideHint = () -> { if (hint != null) fade(hint, false); };

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        setVolumeControlStream(AudioManager.STREAM_MUSIC);
        fill = getPreferences(MODE_PRIVATE).getBoolean("fill", true);
        density = getResources().getConfiguration().densityDpi;
        getWindow().setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE);
        showPairing(null, null);
        acceptIntent(getIntent());
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        acceptIntent(intent);
    }

    private void acceptIntent(Intent intent) {
        if (intent != null && intent.getData() != null) {
            // External QR links prefill pairing; they never start a connection silently.
            showPairing(intent.getData().toString(), null);
            intent.setData(null);
        }
    }

    private int dp(float value) { return Ui.dp(this, value); }

    private LinearLayout column() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(Ui.PANEL);
        root.setPadding(dp(20), dp(24), dp(20), dp(16));
        root.setOnApplyWindowInsetsListener((v, insets) -> {
            int[] edge = Ui.insets(insets);
            v.setPadding(dp(20) + edge[0], dp(24) + edge[1], dp(20) + edge[2], dp(16) + edge[3]);
            return insets;
        });
        return root;
    }

    private void showPairing(String invite, String notice) {
        if (nearby != null) nearby.stop();
        if (surface != null) { surface.removeCallbacks(hideControls); surface.removeCallbacks(hideHint); }
        fullscreen = false; statsOpen = false;
        stopSession();
        updateBackCallback();
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        LinearLayout root = column();
        TextView title = Ui.text(this, "Pluriview Viewer", 26, Ui.GOLD);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        root.addView(title);
        root.addView(Ui.text(this, "See and steer your PC canvas from your phone.", 15, Ui.SECONDARY), Ui.spaced(this, 4));
        if (notice != null) {
            LinearLayout card = Ui.card(this, LinearLayout.VERTICAL);
            card.setBackground(Ui.rounded(Ui.SURFACE, 16, Ui.DANGER, this));
            card.addView(Ui.text(this, notice, 15, Ui.TEXT));
            root.addView(card, Ui.spaced(this, 20));
        }
        nearby = new PairingScreen(this, root, this::openViewer);

        root.addView(Ui.label(this, "Pairing link"), Ui.spaced(this, 28));
        root.addView(Ui.text(this, "Scan the QR code in Tools → Phone Viewer on the PC with your camera, or copy its link and paste it here. A link works for one phone.", 14, Ui.SECONDARY), Ui.spaced(this, 6));
        LinearLayout linkRow = new LinearLayout(this); linkRow.setGravity(Gravity.CENTER_VERTICAL);
        pairing = new EditText(this);
        pairing.setHint("pluriview://viewer?…"); pairing.setTextColor(Ui.TEXT); pairing.setHintTextColor(Ui.SECONDARY);
        pairing.setSingleLine(true); pairing.setTextSize(15);
        pairing.setBackground(Ui.rounded(Ui.SURFACE, 12, Ui.BORDER, this));
        pairing.setPadding(dp(14), 0, dp(14), 0);
        // The link contains the pairing secret; keep it masked like a password.
        pairing.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        pairing.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        pairing.setContentDescription("Pairing link");
        linkRow.addView(pairing, new LinearLayout.LayoutParams(0, dp(48), 1));
        Button paste = Ui.button(this, "Paste", Ui.Kind.SECONDARY);
        LinearLayout.LayoutParams pasteParams = new LinearLayout.LayoutParams(-2, dp(48));
        pasteParams.leftMargin = dp(8);
        linkRow.addView(paste, pasteParams);
        root.addView(linkRow, Ui.spaced(this, 12));
        linkStatus = Ui.text(this, "", 14, Ui.SECONDARY);
        root.addView(linkStatus, Ui.spaced(this, 8));
        connectLink = Ui.button(this, "Connect", Ui.Kind.PRIMARY);
        root.addView(connectLink, Ui.spaced(this, 12));
        root.addView(Ui.text(this, "Use only trusted private Wi-Fi: the stream isn't encrypted. Phone audio starts off.", 13, Ui.SECONDARY), Ui.spaced(this, 28));

        pairing.addTextChangedListener(new TextWatcher() {
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
            public void onTextChanged(CharSequence s, int start, int before, int count) {}
            public void afterTextChanged(Editable s) { describeLink(); }
        });
        paste.setOnClickListener(v -> pasteLink());
        connectLink.setOnClickListener(v -> {
            Link link = Link.parse(pairing.getText().toString().trim());
            if (link != null) openViewer(link.ip, link.port, link.token, UUID.randomUUID().toString().replace("-", ""), null);
        });
        if (invite != null) pairing.setText(invite);
        describeLink();

        ScrollView scroll = new ScrollView(this); scroll.setFillViewport(true); scroll.addView(root);
        scroll.setBackgroundColor(Ui.PANEL);
        setContentView(scroll);
        applySystemBars();
        if (foreground) nearby.start();
    }

    /** A validated pairing link. Only literal private IPv4 hosts are accepted. */
    static final class Link {
        final String ip, token; final int port;
        private Link(String ip, int port, String token) { this.ip = ip; this.port = port; this.token = token; }
        static Link parse(String invite) {
            try {
                Uri uri = Uri.parse(invite);
                String ip = uri.getQueryParameter("host");
                String token = uri.getQueryParameter("token");
                int port = Integer.parseInt(uri.getQueryParameter("port"));
                if (!"pluriview".equals(uri.getScheme()) || !"viewer".equals(uri.getHost())
                        || !privateIpv4(ip) || port < 1 || port > 65535 || token == null || !token.matches("[0-9a-f]{32}"))
                    return null;
                return new Link(ip, port, token);
            } catch (RuntimeException invalid) { return null; }
        }
    }

    private void describeLink() {
        String text = pairing.getText().toString().trim();
        Link link = Link.parse(text);
        Ui.enable(connectLink, link != null);
        if (text.isEmpty()) { linkStatus.setVisibility(View.GONE); return; }
        linkStatus.setVisibility(View.VISIBLE);
        linkStatus.setTextColor(link != null ? Ui.LIVE : Ui.DANGER);
        linkStatus.setText(link != null ? "Ready to connect to the PC at " + link.ip + "." : "That isn't a valid pairing link. Copy a fresh one from Phone Viewer on the PC.");
    }

    private void pasteLink() {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        ClipData clip = clipboard == null ? null : clipboard.getPrimaryClip();
        CharSequence text = clip == null || clip.getItemCount() == 0 ? null : clip.getItemAt(0).coerceToText(this);
        if (text == null || text.toString().trim().isEmpty()) {
            linkStatus.setVisibility(View.VISIBLE); linkStatus.setTextColor(Ui.DANGER);
            linkStatus.setText("The clipboard is empty. Copy the link from Phone Viewer on the PC first.");
            return;
        }
        pairing.setText(text.toString().trim());
    }

    private void openViewer(String ip, int port, String token, String client, String name) {
        if (nearby != null) nearby.stop();
        stopSession();
        Session next = new Session(ip, port, token, client, name != null ? name : ip);
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
        surface = new FrameLayout(this);
        surface.setBackgroundColor(Color.BLACK);
        canvas = new CanvasView(next);
        surface.addView(canvas, new FrameLayout.LayoutParams(-1, -1));
        buildChrome(next);
        surface.setOnApplyWindowInsetsListener((v, insets) -> {
            int[] edge = Ui.insets(insets);
            // Windowed viewing keeps the picture clear of the system bars;
            // fullscreen uses the whole panel, including cutout space.
            FrameLayout.LayoutParams picture = (FrameLayout.LayoutParams) canvas.getLayoutParams();
            int[] margin = fullscreen ? new int[4] : edge;
            if (picture.leftMargin != margin[0] || picture.topMargin != margin[1] || picture.rightMargin != margin[2] || picture.bottomMargin != margin[3]) {
                picture.setMargins(margin[0], margin[1], margin[2], margin[3]); canvas.setLayoutParams(picture);
            }
            chrome.setPadding(dp(12) + edge[0], dp(12) + edge[1], dp(12) + edge[2], dp(16) + edge[3]);
            return insets;
        });
        setContentView(surface);
        setFullscreen(true);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        updateBackCallback();
    }

    /** The control layer over the canvas. Rebuilt without touching the video if display density changes. */
    private void buildChrome(Session next) {
        if (chrome != null && chrome.getParent() == surface) surface.removeView(chrome);
        chrome = new FrameLayout(this);
        surface.addView(chrome, new FrameLayout.LayoutParams(-1, -1));

        // Top left: connection chip, with the optional stats card below it.
        topLeft = new LinearLayout(this); topLeft.setOrientation(LinearLayout.VERTICAL);
        chip = new LinearLayout(this); chip.setGravity(Gravity.CENTER_VERTICAL);
        chip.setBackground(Ui.pressable(this, Ui.SCRIM, 18, Ui.BORDER));
        chip.setPadding(dp(12), dp(8), dp(14), dp(8));
        chipDot = new View(this);
        chip.addView(chipDot, new LinearLayout.LayoutParams(dp(9), dp(9)));
        chipText = Ui.text(this, "", 14, Ui.TEXT);
        LinearLayout.LayoutParams chipLabel = new LinearLayout.LayoutParams(-2, -2);
        chipLabel.leftMargin = dp(8);
        chip.addView(chipText, chipLabel);
        chip.setOnClickListener(v -> toggleStats());
        topLeft.addView(chip, new LinearLayout.LayoutParams(-2, -2));
        statsCard = Ui.card(this, LinearLayout.VERTICAL);
        statsCard.setBackground(Ui.rounded(Ui.SCRIM, 14, Ui.BORDER, this));
        statsCard.setPadding(dp(14), dp(10), dp(14), dp(10));
        statsText = Ui.text(this, "", 13, Ui.TEXT);
        statsText.setTypeface(Typeface.MONOSPACE);
        statsCard.addView(statsText);
        statsCard.setVisibility(View.GONE);
        LinearLayout.LayoutParams statsParams = new LinearLayout.LayoutParams(-2, -2);
        statsParams.topMargin = dp(8);
        topLeft.addView(statsCard, statsParams);
        chrome.addView(topLeft, new FrameLayout.LayoutParams(-2, -2, Gravity.TOP | Gravity.START));

        // Bottom centre: audio status above one pill of icon controls.
        bottom = new LinearLayout(this); bottom.setOrientation(LinearLayout.VERTICAL); bottom.setGravity(Gravity.CENTER_HORIZONTAL);
        audioNote = Ui.text(this, "", 13, Ui.TEXT);
        audioNote.setBackground(Ui.rounded(Ui.SCRIM, 14, Ui.BORDER, this));
        audioNote.setPadding(dp(12), dp(6), dp(12), dp(6));
        LinearLayout.LayoutParams noteParams = new LinearLayout.LayoutParams(-2, -2);
        noteParams.bottomMargin = dp(8);
        bottom.addView(audioNote, noteParams);
        LinearLayout pill = new LinearLayout(this); pill.setGravity(Gravity.CENTER_VERTICAL);
        pill.setBackground(Ui.rounded(Ui.SCRIM, 28, Ui.BORDER, this));
        pill.setPadding(dp(4), dp(2), dp(4), dp(2));
        fitButton = Ui.icon(this, R.drawable.ic_fill, "");
        fitButton.setOnClickListener(v -> toggleFit());
        audioButton = Ui.icon(this, R.drawable.ic_volume_off, "");
        audioButton.setOnClickListener(v -> next.toggleAudio());
        statsButton = Ui.icon(this, R.drawable.ic_info, "Connection details");
        statsButton.setOnClickListener(v -> toggleStats());
        fullscreenButton = Ui.icon(this, R.drawable.ic_fullscreen, "");
        fullscreenButton.setOnClickListener(v -> setFullscreen(!fullscreen));
        ImageButton disconnect = Ui.icon(this, R.drawable.ic_disconnect, "Disconnect");
        disconnect.setOnClickListener(v -> confirmDisconnect());
        for (ImageButton button : new ImageButton[] {fitButton, audioButton, statsButton, fullscreenButton, disconnect})
            pill.addView(button, new LinearLayout.LayoutParams(dp(48), dp(48)));
        bottom.addView(pill, new LinearLayout.LayoutParams(-2, -2));
        chrome.addView(bottom, new FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM | Gravity.CENTER_HORIZONTAL));

        // Centre: shown whenever there is no picture, so status is never hidden with the controls.
        statePanel = Ui.card(this, LinearLayout.VERTICAL);
        statePanel.setBackground(Ui.rounded(Ui.SCRIM, 18, Ui.BORDER, this));
        statePanel.setGravity(Gravity.CENTER_HORIZONTAL);
        statePanel.setPadding(dp(24), dp(20), dp(24), dp(20));
        stateSpinner = Ui.spinner(this, 28);
        statePanel.addView(stateSpinner);
        stateTitle = Ui.text(this, "", 17, Ui.TEXT); stateTitle.setGravity(Gravity.CENTER);
        stateTitle.setTypeface(Typeface.DEFAULT_BOLD); stateTitle.setMaxWidth(dp(300));
        statePanel.addView(stateTitle, Ui.spaced(this, 12));
        stateDetail = Ui.text(this, "", 14, Ui.SECONDARY); stateDetail.setGravity(Gravity.CENTER); stateDetail.setMaxWidth(dp(300));
        statePanel.addView(stateDetail, Ui.spaced(this, 6));
        stateAction = Ui.button(this, "Back to pairing", Ui.Kind.SECONDARY);
        stateAction.setOnClickListener(v -> showPairing(null, null));
        statePanel.addView(stateAction, Ui.spaced(this, 16));
        // Wrap content: the text's maximum width keeps the panel within narrow screens after rotation.
        chrome.addView(statePanel, new FrameLayout.LayoutParams(-2, -2, Gravity.CENTER));

        hint = Ui.card(this, LinearLayout.VERTICAL);
        hint.setBackground(Ui.rounded(Ui.SCRIM, 18, Ui.GOLD, this));
        TextView hintText = Ui.text(this, "Drag to pan · Pinch to zoom\nDouble-tap a tile to focus it", 15, Ui.TEXT);
        hintText.setGravity(Gravity.CENTER);
        hint.addView(hintText);
        hint.setVisibility(View.GONE);
        chrome.addView(hint, new FrameLayout.LayoutParams(-2, -2, Gravity.CENTER));

        statsButton.setImageTintList(ColorStateList.valueOf(statsOpen ? Ui.GOLD : Ui.TEXT));
        fullscreenButton.setImageResource(fullscreen ? R.drawable.ic_fullscreen_exit : R.drawable.ic_fullscreen);
        Ui.describe(fullscreenButton, fullscreen ? "Exit full screen" : "Full screen");
        updateFitLabels();
        next.updateAudioLabels();
        next.updateStats();
        renderState();
        if (!controlsShown) { bottom.setVisibility(View.GONE); statsCard.setVisibility(View.GONE); }
    }

    @Override public void onConfigurationChanged(Configuration config) {
        super.onConfigurationChanged(config);
        if (config.densityDpi == density) return;
        density = config.densityDpi;
        // Views were sized in pixels for the old density. Rebuild them, keeping the session.
        if (session != null && surface != null) { buildChrome(session); surface.requestApplyInsets(); }
        else if (pairing != null) showPairing(pairing.getText().toString(), null);
    }

    private void fade(View view, boolean show) {
        if (show) {
            if (view.getVisibility() != View.VISIBLE) { view.setAlpha(0); view.setVisibility(View.VISIBLE); }
            view.animate().alpha(1).setDuration(150).start();
        } else if (view.getVisibility() == View.VISIBLE) {
            // A cancelled fade-out never runs its end action, so showing again wins.
            view.animate().alpha(0).setDuration(200).withEndAction(() -> view.setVisibility(View.GONE)).start();
        }
    }

    private void setControls(boolean show) {
        controlsShown = show;
        fade(bottom, show);
        fade(topLeft, show || (session != null && session.state != State.LIVE));
        fade(statsCard, show && statsOpen);
    }

    /** Show controls; in fullscreen they hide again after a few seconds without use. */
    private void revealControls() {
        if (surface == null || session == null) return;
        setControls(true);
        surface.removeCallbacks(hideControls);
        if (fullscreen) surface.postDelayed(hideControls, 3500);
    }

    /** A tap on the picture shows or hides the controls, in fullscreen and windowed viewing. */
    private void toggleControls() {
        if (controlsShown) { surface.removeCallbacks(hideControls); setControls(false); }
        else revealControls();
    }

    private void toggleStats() {
        statsOpen = !statsOpen;
        statsButton.setImageTintList(ColorStateList.valueOf(statsOpen ? Ui.GOLD : Ui.TEXT));
        if (session != null) session.updateStats();
        revealControls();
    }

    private void updateFitLabels() {
        if (fitButton == null) return;
        fitButton.setImageResource(fill ? R.drawable.ic_fill : R.drawable.ic_fit);
        Ui.describe(fitButton, fill ? "Filling the screen. Tap to fit the whole canvas" : "Fitting the whole canvas. Tap to fill the screen");
    }
    private void toggleFit() {
        fill = !fill; getPreferences(MODE_PRIVATE).edit().putBoolean("fill", fill).apply();
        updateFitLabels(); canvas.resetGesture(); canvas.layoutVideo(); canvas.invalidate(); revealControls();
    }
    private void setFullscreen(boolean value) {
        fullscreen = value;
        fullscreenButton.setImageResource(value ? R.drawable.ic_fullscreen_exit : R.drawable.ic_fullscreen);
        Ui.describe(fullscreenButton, value ? "Exit full screen" : "Full screen");
        applySystemBars(); surface.requestApplyInsets(); canvas.resetGesture();
        revealControls();
    }

    private void confirmDisconnect() {
        Session current = session;
        if (current == null) return;
        new AlertDialog.Builder(this)
            .setTitle("Disconnect from " + current.name + "?")
            .setMessage("To view again, you'll need to request access and approve it on the PC.")
            .setPositiveButton("Disconnect", (dialog, which) -> { if (session == current) showPairing(null, null); })
            .setNegativeButton("Keep viewing", null)
            .show();
    }

    /** Back leaves fullscreen first, then asks before disconnecting. */
    private boolean handleBack() {
        if (session == null) return false;
        if (fullscreen) setFullscreen(false); else confirmDisconnect();
        return true;
    }

    @Override public void onBackPressed() { if (!handleBack()) super.onBackPressed(); }

    /** Android 13+ predictive back: intercept only while viewing, so the pairing screen keeps the system animation. */
    private void updateBackCallback() {
        if (Build.VERSION.SDK_INT < 33) return;
        boolean wanted = session != null;
        if (wanted && backCallback == null) backCallback = BackCallback.register(this, this::handleBack);
        else if (!wanted && backCallback != null) { BackCallback.unregister(this, backCallback); backCallback = null; }
    }

    private static final class BackCallback {
        static Object register(Activity activity, Runnable action) {
            android.window.OnBackInvokedCallback callback = action::run;
            activity.getOnBackInvokedDispatcher().registerOnBackInvokedCallback(android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, callback);
            return callback;
        }
        static void unregister(Activity activity, Object callback) {
            activity.getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback((android.window.OnBackInvokedCallback) callback);
        }
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

    /** Draw the chip and centre panel from the session state. */
    private void renderState() {
        Session current = session;
        if (current == null || canvas == null || canvas.owner != current) return;
        State state = current.state;
        int color = state == State.LIVE ? Ui.LIVE : state == State.ERROR ? Ui.DANGER : Ui.GOLD;
        chipDot.setBackground(Ui.rounded(color, 5, 0, this));
        String label = state == State.LIVE ? "Live" : state == State.CONNECTING ? "Connecting"
            : state == State.RECOVERING ? "Reconnecting" : state == State.WAITING ? "Paused" : "Not connected";
        chipText.setText(label + " · " + current.name);
        Ui.describe(chip, label + ", " + current.name + ". Tap for connection details");
        fade(topLeft, controlsShown || state != State.LIVE);
        boolean picture = canvas.hasPicture();
        statePanel.setVisibility(picture ? View.GONE : View.VISIBLE);
        stateTitle.setText(current.stateTitle);
        stateDetail.setText(current.stateDetail);
        stateDetail.setVisibility(current.stateDetail.isEmpty() ? View.GONE : View.VISIBLE);
        stateSpinner.setVisibility(state == State.ERROR ? View.GONE : View.VISIBLE);
        stateAction.setVisibility(state == State.ERROR ? View.VISIBLE : View.GONE);
    }

    private void maybeShowHint() {
        if (getPreferences(MODE_PRIVATE).getBoolean(HINT_SHOWN, false) || hint == null) return;
        getPreferences(MODE_PRIVATE).edit().putBoolean(HINT_SHOWN, true).apply();
        fade(hint, true);
        surface.removeCallbacks(hideHint);
        surface.postDelayed(hideHint, 5000);
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

    @Override protected void onDestroy() {
        if (nearby != null) nearby.stop();
        stopSession();
        if (backCallback != null) { BackCallback.unregister(this, backCallback); backCallback = null; }
        super.onDestroy();
    }
    private void stopSession() { if (session != null) session.pause(); session = null; }

    private final class Session {
        static final String MISSING_FRAMES = "Missing video frames", DRIVER_STUCK = "Video driver did not stop";
        final String base, token, client, name;
        final CameraTransform camera = new CameraTransform();
        final AtomicReference<float[]> pendingFocus = new AtomicReference<>();
        final AtomicBoolean running = new AtomicBoolean(false);
        /** Wakes the control worker when a gesture or focus command is waiting. */
        final Object commands = new Object();
        volatile boolean ready;
        volatile float width, height;
        volatile int generation;
        ExecutorService workers;
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
        /** Video is briefly recovering (Wi-Fi hiccup), not paused or stopped. */
        volatile boolean videoHeld;
        // Read on the UI thread only.
        State state = State.CONNECTING;
        String stateTitle = "", stateDetail = "";
        boolean audioEnabled;
        AudioStream audioStream;
        AudioFocusRequest audioFocus;
        int audioRevision;
        int audioFocusRevision;
        String audioText = "Audio: off";

        Session(String ip, int port, String token, String client, String name) {
            base = "http://" + ip + ":" + port; this.token = token; this.client = client; this.name = name;
            stateTitle = "Connecting to " + name;
        }

        void updateAudioLabels() {
            if (session != this || audioButton == null) return;
            audioButton.setImageResource(audioEnabled ? R.drawable.ic_volume_on : R.drawable.ic_volume_off);
            audioButton.setImageTintList(ColorStateList.valueOf(audioEnabled ? Ui.GOLD : Ui.TEXT));
            Ui.describe(audioButton, audioEnabled ? "Phone audio is on. Tap to turn it off" : "Phone audio is off. Tap to turn it on");
            boolean note = (audioEnabled && !AUDIO_PLAYING.equals(audioText)) || audioText.startsWith("Audio off:");
            audioNote.setText(audioText);
            audioNote.setVisibility(note ? View.VISIBLE : View.GONE);
        }

        void toggleAudio() {
            audioEnabled = !audioEnabled;
            audioText = audioEnabled ? "Audio: waiting for canvas" : "Audio: off";
            updateAudio(); updateAudioLabels(); revealControls();
        }

        /** Explain why audio switched itself off, then clear the note. */
        void audioOff(String reason) {
            audioEnabled = false; stopAudio(); audioText = reason; updateAudioLabels(); revealControls();
            canvas.postDelayed(() -> {
                if (session == this && !audioEnabled && reason.equals(audioText)) { audioText = "Audio: off"; updateAudioLabels(); }
            }, 6000);
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
            // Keep playing while video briefly recovers: restarting audio takes
            // seconds, and the PC ends audio itself on a real pause or loss.
            if (!audioEnabled || !running.get() || !foreground || (!ready && !(videoHeld && audioStream != null))) {
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
                        if (change < 0 && session == this && audioFocusRevision == focusRevision) audioOff("Audio off: another app is playing sound");
                    }).build();
                if (manager.requestAudioFocus(audioFocus) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
                    audioOff("Audio off: sound is in use by another app"); return;
                }
            }
            final int revision = ++audioRevision;
            try {
                audioStream = new AudioStream(base, token, client, audioNetworkSlot, audioCodecSlot, value -> runOnUiThread(() -> {
                    if (session == this && audioRevision == revision && audioEnabled && running.get() && !value.equals(audioText)) {
                        audioText = value; updateAudioLabels(); updateStats();
                    }
                }));
            } catch (java.io.IOException error) { stopAudio(); audioText = "Audio: waiting for previous playback to stop"; }
            updateAudioLabels();
        }

        void start() {
            if (!running.compareAndSet(false, true)) return;
            final int run = ++generation;
            ready = false; camera.clear(); latestVideo = null;
            recovery.reset(); reconnects = 0; lastRecovery = "";
            receivedRate.clear(); shownRate.clear();
            message(run, State.CONNECTING, "Connecting to " + name, "Starting 720p video…", true);
            workers = Executors.newFixedThreadPool(2);
            workers.execute(() -> streamVideo(run));
            workers.execute(() -> sendCommands(run));
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
            if (canvas != null && canvas.owner == this) { canvas.hidePicture(); canvas.resetGesture(); }
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

        void message(int run, State next, String title, String detail, boolean clear) {
            if (!current(run)) return;
            if (clear) { recovery.reset(); ready = false; videoHeld = false; camera.clear(); pendingFocus.set(null); latestVideo = null; }
            runOnUiThread(() -> {
                if (!current(run) || session != this) return;
                state = next; stateTitle = title; stateDetail = detail;
                if (clear) { canvas.hidePicture(); canvas.resetGesture(); updateAudio(); }
                renderState(); updateStats();
            });
        }

        /** The PC no longer accepts this phone's credentials; retrying cannot help. */
        void accessEnded(int run) {
            runOnUiThread(() -> {
                if (current(run) && session == this)
                    showPairing(null, "This phone's access to " + name + " has ended, for example because sharing stopped or a new pairing link was made. Request access again.");
            });
        }

        void recoverVideo(int run, Exception error) {
            if (!current(run)) return;
            // Invalidate callbacks from the outgoing decoder before retaining its
            // last texture. A reconnect must not keep accepting stale gestures.
            decoderRevision++;
            reconnects++;
            lastRecovery = recoveryReason(error);
            holdVideo(run, "Reconnecting video", lastRecovery + " · reconnect " + reconnects);
        }

        void holdVideo(int run, String title, String detail) {
            if (!current(run)) return;
            ready = false; videoHeld = true; camera.clear(); pendingFocus.set(null); latestVideo = null;
            message(run, State.RECOVERING, title, detail, false);
            if (recovery.begin(System.nanoTime())) {
                runOnUiThread(() -> {
                    if (!current(run) || session != this) return;
                    canvas.resetGesture();
                    canvas.postDelayed(() -> {
                        if (current(run) && session == this && recovery.expired(System.nanoTime())) canvas.hidePicture();
                    }, 1000);
                });
            }
        }

        String recoveryReason(Exception error) {
            // Only fixed labels enter the UI; never echo URLs, credentials or
            // arbitrary server/driver error text.
            if (error instanceof java.net.SocketTimeoutException)
                return VideoConnection.RESPONSE_TIMEOUT.equals(error.getMessage()) ? "Connection response timeout" : "Video data timeout";
            if (error instanceof java.io.EOFException) return "PC stream ended";
            String detail = error.getMessage();
            if (VideoDecoder.BACKLOG.equals(detail)) return "Decoder queue full";
            if (VideoDecoder.STALLED.equals(detail)) return "Decoder stalled";
            if (VideoDecoder.RESTART.equals(detail)) return "Decoder reset";
            if (MISSING_FRAMES.equals(detail)) return "Missing video frames";
            if (DRIVER_STUCK.equals(detail)) return "Decoder stopping";
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
                width = newest.viewportWidth; height = newest.viewportHeight; ready = true; videoHeld = false;
                boolean wasLive = state == State.LIVE;
                state = State.LIVE;
                canvas.showVideo(newest.width, newest.height);
                if (!wasLive) { renderState(); maybeShowHint(); }
            });
        }

        void updateStats() {
            if (session != this || statsText == null) return;
            StringBuilder text = new StringBuilder();
            VideoPacket newest = latestVideo;
            if (ready && newest != null) {
                long now = System.nanoTime();
                text.append("Video    ").append(newest.width).append('×').append(newest.height)
                    .append("\nReceived ").append(receivedRate.fps(now)).append(" FPS")
                    .append("\nShown    ").append(shownRate.fps(now)).append(" FPS");
            } else text.append("Video    not playing");
            if (reconnects > 0) text.append("\nReconnects ").append(reconnects).append(" · ").append(lastRecovery);
            text.append('\n').append(audioText);
            statsText.setText(text);
            if (controlsShown && statsOpen && statsCard.getVisibility() != View.VISIBLE) fade(statsCard, true);
            if (!statsOpen && statsCard.getVisibility() == View.VISIBLE) fade(statsCard, false);
        }

        void streamVideo(int run) {
            while (current(run)) {
                if (!decoderSlot.available()) {
                    message(run, State.CONNECTING, "Getting ready", "Waiting for the previous video decoder to stop…", true);
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
                    connection = new VideoConnection(base, token, client, () -> holdVideo(run, "Waiting for video data", ""));
                    if (!current(run)) return;
                    videoConnection = connection;
                    int code = connection.status;
                    if (code == 404) { message(run, State.ERROR, "Update Pluriview on the PC", "This PC version can't stream video to the phone.", true); return; }
                    if (code == 401) { accessEnded(run); return; }
                    if (code == 409) { message(run, State.CONNECTING, "Waiting for the PC", "The previous viewer connection is still closing…", false); if (!delay(run, 500)) return; continue; }
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
                                if (packet.kind == 3) { message(run, State.WAITING, "PC video encoder unavailable", "Restart Phone Viewer sharing on the PC. Retrying…", true); retryMillis = 2000; break; }
                                message(run, State.WAITING, "Paused on the PC", "Close dialogs and menus on the PC, or leave browser interaction, to resume.", true);
                                continue;
                            }
                            if (sequence != 0 && packet.sequence != sequence + 1) throw new java.io.IOException(MISSING_FRAMES);
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
                    message(run, State.WAITING, "Video decoder unavailable", "This phone couldn't start its video decoder. Retrying…", true);
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
            if (!decoder.terminated()) throw new java.io.IOException(DRIVER_STUCK);
        }

        void wake() { synchronized (commands) { commands.notifyAll(); } }

        boolean commandWaiting() { return ready && (pendingFocus.get() != null || camera.pending()); }

        /** Sleeps until a gesture arrives, instead of polling; wakes each second to keep the control connection alive. */
        void sendCommands(int run) {
            try {
                while (current(run)) {
                    sendCamera(run);
                    // Coalesce gesture updates arriving within one send interval.
                    Thread.sleep(8);
                    synchronized (commands) { if (current(run) && !commandWaiting()) commands.wait(1000); }
                }
            } catch (InterruptedException stopped) { Thread.currentThread().interrupt(); }
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
        final Runnable tapControls = () -> toggleControls();
        /** Expanding ring that confirms a double-tap focus request. */
        final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
        float ringX, ringY;
        long ringStart;

        CanvasView(Session owner) {
            super(ViewerActivity.this); this.owner = owner; setWillNotDraw(false);
            setContentDescription("Live PC canvas. Drag to pan, pinch to zoom, double-tap a tile to toggle focus.");
            ring.setStyle(Paint.Style.STROKE); ring.setStrokeWidth(dp(3)); ring.setColor(Ui.GOLD);
            video = new TextureView(ViewerActivity.this); video.setAlpha(0);
            addView(video, new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER));
            video.setSurfaceTextureListener(new TextureView.SurfaceTextureListener() {
                @Override public void onSurfaceTextureAvailable(SurfaceTexture value, int w, int h) { texture = value; }
                @Override public void onSurfaceTextureSizeChanged(SurfaceTexture value, int w, int h) {}
                @Override public void onSurfaceTextureUpdated(SurfaceTexture value) { if (owner.current(owner.generation)) owner.shownRate.record(System.nanoTime()); }
                @Override public boolean onSurfaceTextureDestroyed(SurfaceTexture value) { texture = null; owner.ready = false; resetGesture(); return true; }
            });
        }

        boolean hasPicture() { return video.getAlpha() != 0; }

        void showVideo(int w, int h) {
            if (videoWidth != w || videoHeight != h || video.getAlpha() == 0) {
                boolean appeared = video.getAlpha() == 0;
                videoWidth = w; videoHeight = h; layoutVideo(); video.setAlpha(1); invalidate();
                if (appeared) renderState();
            }
        }

        void hidePicture() {
            if (video.getAlpha() == 0) return;
            video.setAlpha(0); invalidate(); renderState();
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

        @Override protected void dispatchDraw(Canvas c) {
            super.dispatchDraw(c);
            float progress = (System.nanoTime() - ringStart) / 350_000_000f;
            if (ringStart == 0 || progress >= 1) return;
            ring.setAlpha((int) (255 * (1 - progress)));
            c.drawCircle(ringX, ringY, dp(14) + dp(26) * progress, ring);
            postInvalidateOnAnimation();
        }

        void confirmFocus(float x, float y) {
            ringX = x; ringY = y; ringStart = System.nanoTime(); postInvalidateOnAnimation();
            performHapticFeedback(Build.VERSION.SDK_INT >= 30 ? HapticFeedbackConstants.CONFIRM : HapticFeedbackConstants.VIRTUAL_KEY);
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
                if (action == MotionEvent.ACTION_UP) toggleControls();
                return true;
            }
            if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
                if (action == MotionEvent.ACTION_UP && !moved) {
                    removeCallbacks(tapControls);
                    if (taps.up(event.getDownTime(), event.getEventTime(), event.getX(), event.getY())) {
                        owner.camera.clear();
                        owner.pendingFocus.set(new float[]{(event.getX() - display.left) / display.width(), (event.getY() - display.top) / display.height(), owner.width, owner.height});
                        owner.wake();
                        confirmFocus(event.getX(), event.getY());
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
                owner.wake();
            }
            previousX = x; previousY = y; previousSpan = span; pointers = count;
            return true;
        }
    }
}
