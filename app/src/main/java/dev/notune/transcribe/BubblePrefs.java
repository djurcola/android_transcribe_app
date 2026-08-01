package dev.notune.transcribe;

import android.content.Context;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

public final class BubblePrefs {
    private static final String POS_X_FILE = "bubble_x";
    private static final String POS_Y_FILE = "bubble_y";
    private static final String ENABLED_FILE = "bubble_enabled";
    private static final String A11Y_CONSENT_FILE = "a11y_insertion_consent";

    private BubblePrefs() {}

    public static int getX(Context ctx) {
        return readInt(ctx, POS_X_FILE, -1);
    }

    public static void setX(Context ctx, int x) {
        writeInt(ctx, POS_X_FILE, x);
    }

    public static int getY(Context ctx) {
        return readInt(ctx, POS_Y_FILE, -1);
    }

    public static void setY(Context ctx, int y) {
        writeInt(ctx, POS_Y_FILE, y);
    }

    public static boolean isEnabled(Context ctx) {
        return new File(ctx.getFilesDir(), ENABLED_FILE).exists();
    }

    public static void setEnabled(Context ctx, boolean enabled) {
        File f = new File(ctx.getFilesDir(), ENABLED_FILE);
        if (enabled) {
            try { f.createNewFile(); } catch (IOException ignored) { }
        } else {
            f.delete();
        }
    }

    public static boolean hasA11yConsent(Context ctx) {
        return new File(ctx.getFilesDir(), A11Y_CONSENT_FILE).exists();
    }

    public static void setA11yConsent(Context ctx, boolean consent) {
        File f = new File(ctx.getFilesDir(), A11Y_CONSENT_FILE);
        if (consent) {
            try { f.createNewFile(); } catch (IOException ignored) { }
        } else {
            f.delete();
        }
    }

    private static int readInt(Context ctx, String name, int def) {
        File f = new File(ctx.getFilesDir(), name);
        if (!f.exists()) return def;
        try {
            return Integer.parseInt(
                    new String(Files.readAllBytes(f.toPath()), StandardCharsets.UTF_8).trim());
        } catch (IOException | NumberFormatException e) {
            return def;
        }
    }

    private static void writeInt(Context ctx, String name, int value) {
        File f = new File(ctx.getFilesDir(), name);
        try {
            Files.write(f.toPath(), String.valueOf(value).getBytes(StandardCharsets.UTF_8));
        } catch (IOException ignored) { }
    }
}
