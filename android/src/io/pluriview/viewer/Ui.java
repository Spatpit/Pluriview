package io.pluriview.viewer;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.os.Build;
import android.view.Gravity;
import android.view.View;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;

/** Code-built widgets in the desktop palette (src/ui_theme.rs); no layout XML or libraries. */
final class Ui {
    static final int PANEL = Color.rgb(22, 23, 27);
    static final int SURFACE = Color.rgb(30, 31, 36);
    static final int BORDER = Color.rgb(61, 61, 66);
    static final int TEXT = Color.rgb(235, 233, 227);
    static final int SECONDARY = Color.rgb(170, 172, 180);
    static final int GOLD = Color.rgb(224, 181, 83);
    static final int GOLD_DARK = Color.rgb(65, 53, 30);
    static final int DANGER = Color.rgb(244, 151, 146);
    static final int LIVE = Color.rgb(126, 204, 140);
    /** Translucent panel for controls drawn over the live canvas. */
    static final int SCRIM = 0xE61E1F24;

    enum Kind { PRIMARY, SECONDARY, QUIET }

    private Ui() {}

    static int dp(Context context, float value) { return (int) (value * context.getResources().getDisplayMetrics().density + .5f); }

    static TextView text(Context context, String value, float sp, int color) {
        TextView view = new TextView(context);
        view.setText(value); view.setTextSize(sp); view.setTextColor(color);
        view.setLineSpacing(0, 1.15f);
        return view;
    }

    static TextView label(Context context, String value) {
        TextView view = text(context, value.toUpperCase(java.util.Locale.ROOT), 12, SECONDARY);
        view.setLetterSpacing(.08f); view.setTypeface(Typeface.DEFAULT_BOLD);
        return view;
    }

    static GradientDrawable rounded(int fill, float radiusDp, int stroke, Context context) {
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(fill); shape.setCornerRadius(dp(context, radiusDp));
        if (stroke != 0) shape.setStroke(dp(context, 1), stroke);
        return shape;
    }

    static Drawable pressable(Context context, int fill, float radiusDp, int stroke) {
        return new RippleDrawable(ColorStateList.valueOf(0x33FFFFFF), rounded(fill, radiusDp, stroke, context), rounded(Color.WHITE, radiusDp, 0, context));
    }

    static Button button(Context context, String value, Kind kind) {
        Button button = new Button(context);
        button.setText(value); button.setAllCaps(false); button.setTextSize(15);
        button.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        button.setStateListAnimator(null); button.setElevation(0);
        button.setMinHeight(dp(context, 48)); button.setMinimumHeight(dp(context, 48));
        button.setPadding(dp(context, 18), 0, dp(context, 18), 0);
        if (kind == Kind.PRIMARY) { button.setTextColor(PANEL); button.setBackground(pressable(context, GOLD, 12, 0)); }
        else if (kind == Kind.SECONDARY) { button.setTextColor(TEXT); button.setBackground(pressable(context, SURFACE, 12, BORDER)); }
        else { button.setTextColor(GOLD); button.setBackground(pressable(context, Color.TRANSPARENT, 12, 0)); }
        return button;
    }

    static ImageButton icon(Context context, int drawable, String description) {
        ImageButton button = new ImageButton(context);
        button.setImageResource(drawable);
        button.setImageTintList(ColorStateList.valueOf(TEXT));
        // Unbounded ripple: the pill behind the buttons provides the shape.
        button.setBackground(new RippleDrawable(ColorStateList.valueOf(0x40FFFFFF), null, null));
        int pad = dp(context, 12);
        button.setPadding(pad, pad, pad, pad);
        describe(button, description);
        return button;
    }

    static void describe(View view, String description) {
        view.setContentDescription(description);
        view.setTooltipText(description);
    }

    static void enable(View view, boolean enabled) {
        view.setEnabled(enabled); view.setAlpha(enabled ? 1 : .45f);
    }

    static ProgressBar spinner(Context context, int sizeDp) {
        ProgressBar spinner = new ProgressBar(context);
        spinner.setIndeterminate(true);
        spinner.setIndeterminateTintList(ColorStateList.valueOf(GOLD));
        spinner.setLayoutParams(new LinearLayout.LayoutParams(dp(context, sizeDp), dp(context, sizeDp)));
        return spinner;
    }

    static LinearLayout card(Context context, int orientation) {
        LinearLayout card = new LinearLayout(context);
        card.setOrientation(orientation);
        card.setBackground(rounded(SURFACE, 16, BORDER, context));
        int pad = dp(context, 16);
        card.setPadding(pad, pad, pad, pad);
        if (orientation == LinearLayout.HORIZONTAL) card.setGravity(Gravity.CENTER_VERTICAL);
        return card;
    }

    static LinearLayout.LayoutParams spaced(Context context, int topDp) {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(context, topDp);
        return params;
    }

    /** Left, top, right, bottom area covered by visible system bars, cutouts and the keyboard. */
    static int[] insets(WindowInsets insets) {
        if (Build.VERSION.SDK_INT >= 30) {
            android.graphics.Insets bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            android.graphics.Insets ime = insets.getInsets(WindowInsets.Type.ime());
            return new int[] {bars.left, bars.top, bars.right, Math.max(bars.bottom, ime.bottom)};
        }
        int left = insets.getSystemWindowInsetLeft(), top = insets.getSystemWindowInsetTop();
        int right = insets.getSystemWindowInsetRight(), bottom = insets.getSystemWindowInsetBottom();
        if (Build.VERSION.SDK_INT >= 28 && insets.getDisplayCutout() != null) {
            left = Math.max(left, insets.getDisplayCutout().getSafeInsetLeft());
            top = Math.max(top, insets.getDisplayCutout().getSafeInsetTop());
            right = Math.max(right, insets.getDisplayCutout().getSafeInsetRight());
        }
        return new int[] {left, top, right, bottom};
    }
}
