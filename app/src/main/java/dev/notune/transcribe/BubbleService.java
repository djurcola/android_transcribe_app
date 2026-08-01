package dev.notune.transcribe;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ServiceInfo;
import android.graphics.PixelFormat;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.provider.Settings;
import android.util.Log;
import android.view.Gravity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.ImageView;
import android.widget.Toast;

public class BubbleService extends Service {
    private static final String TAG = "BubbleService";
    public static final String ACTION_SHOW = "dev.notune.transcribe.BUBBLE_SHOW";
    public static final String ACTION_HIDE = "dev.notune.transcribe.BUBBLE_HIDE";
    public static final String ACTION_STOP_RECORDING = "dev.notune.transcribe.BUBBLE_STOP_REC";
    private static final String CHANNEL_ID = "BubbleChannel";
    private static final int NOTIFICATION_ID = 23456;

    static {
        try {
            System.loadLibrary("c++_shared");
            System.loadLibrary("android_transcribe_app");
        } catch (UnsatisfiedLinkError e) {
            Log.e(TAG, "Failed to load native libraries", e);
        }
    }

    private WindowManager mWindowManager;
    private View mBubbleView;
    private ImageView mBubbleIcon;
    private Handler mMainHandler;
    private boolean isRecording = false;
    private boolean isProcessing = false;

    @Override
    public void onCreate() {
        super.onCreate();
        mMainHandler = new Handler(Looper.getMainLooper());
        mWindowManager = (WindowManager) getSystemService(WINDOW_SERVICE);
        createNotificationChannel();
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null) return START_NOT_STICKY;
        String action = intent.getAction();

        if (ACTION_SHOW.equals(action)) {
            if (!Settings.canDrawOverlays(this)) {
                stopSelf();
                return START_NOT_STICKY;
            }
            Notification notification = createNotification();
            try {
                if (Build.VERSION.SDK_INT >= 29) {
                    startForeground(NOTIFICATION_ID, notification,
                            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE);
                } else {
                    startForeground(NOTIFICATION_ID, notification);
                }
            } catch (Exception e) {
                Log.e(TAG, "Failed to start foreground", e);
                stopSelf();
                return START_NOT_STICKY;
            }
            showBubble();
        } else if (ACTION_HIDE.equals(action)) {
            hideBubble();
            stopForeground(STOP_FOREGROUND_REMOVE);
            stopSelf();
        } else if (ACTION_STOP_RECORDING.equals(action)) {
            if (isRecording) {
                stopRecordingFlow();
            }
        }
        return START_NOT_STICKY;
    }

    private void showBubble() {
        if (mBubbleView != null) return;

        mBubbleView = LayoutInflater.from(this).inflate(R.layout.bubble_overlay, null);
        mBubbleIcon = mBubbleView.findViewById(R.id.bubble_icon);

        int layoutFlag = WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY;

        WindowManager.LayoutParams params = new WindowManager.LayoutParams(
                WindowManager.LayoutParams.WRAP_CONTENT,
                WindowManager.LayoutParams.WRAP_CONTENT,
                layoutFlag,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                        | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                PixelFormat.TRANSLUCENT);

        params.gravity = Gravity.TOP | Gravity.START;
        int savedX = BubblePrefs.getX(this);
        int savedY = BubblePrefs.getY(this);
        if (savedX >= 0 && savedY >= 0) {
            params.x = savedX;
            params.y = savedY;
        } else {
            params.x = getResources().getDisplayMetrics().widthPixels - 160;
            params.y = getResources().getDisplayMetrics().heightPixels / 3;
        }

        mWindowManager.addView(mBubbleView, params);
        updateBubbleState();
        makeDraggable(params);

        initNative(this);
    }

    private void makeDraggable(WindowManager.LayoutParams params) {
        mBubbleView.setOnTouchListener(new View.OnTouchListener() {
            private float downX, downY;
            private int downParamX, downParamY;
            private boolean moved;

            @Override
            public boolean onTouch(View v, MotionEvent event) {
                switch (event.getAction()) {
                    case MotionEvent.ACTION_DOWN:
                        downX = event.getRawX();
                        downY = event.getRawY();
                        downParamX = params.x;
                        downParamY = params.y;
                        moved = false;
                        return true;
                    case MotionEvent.ACTION_MOVE: {
                        int dx = (int) (event.getRawX() - downX);
                        int dy = (int) (event.getRawY() - downY);
                        if (Math.abs(dx) > 8 || Math.abs(dy) > 8) moved = true;
                        params.x = downParamX + dx;
                        params.y = downParamY + dy;
                        if (mBubbleView != null) {
                            mWindowManager.updateViewLayout(mBubbleView, params);
                        }
                        return true;
                    }
                    case MotionEvent.ACTION_UP:
                        if (moved) {
                            BubblePrefs.setX(BubbleService.this, params.x);
                            BubblePrefs.setY(BubbleService.this, params.y);
                        } else {
                            onBubbleTap();
                        }
                        return true;
                }
                return false;
            }
        });
    }

    private void onBubbleTap() {
        if (isProcessing) return;
        if (isRecording) {
            stopRecordingFlow();
        } else {
            if (checkSelfPermission(android.Manifest.permission.RECORD_AUDIO)
                    != PackageManager.PERMISSION_GRANTED) {
                Toast.makeText(this, R.string.bubble_need_mic, Toast.LENGTH_SHORT).show();
                return;
            }
            isRecording = true;
            updateBubbleState();
            startRecordingNative();
        }
    }

    private void stopRecordingFlow() {
        isRecording = false;
        isProcessing = true;
        updateBubbleState();
        stopRecordingNative();
    }

    private void updateBubbleState() {
        mMainHandler.post(() -> {
            if (mBubbleIcon == null) return;
            if (isRecording) {
                mBubbleIcon.setImageResource(R.drawable.ic_mic);
                mBubbleView.setBackgroundResource(R.drawable.bg_bubble_recording);
                mBubbleView.setContentDescription(getString(R.string.bubble_recording));
            } else if (isProcessing) {
                mBubbleIcon.setImageResource(R.drawable.ic_mic);
                mBubbleView.setBackgroundResource(R.drawable.bg_bubble_processing);
                mBubbleView.setContentDescription(getString(R.string.bubble_processing));
            } else {
                mBubbleIcon.setImageResource(R.drawable.ic_mic);
                mBubbleView.setBackgroundResource(R.drawable.bg_bubble_idle);
                mBubbleView.setContentDescription(getString(R.string.bubble_idle));
            }
        });
    }

    private void hideBubble() {
        if (mBubbleView != null && mWindowManager != null) {
            mWindowManager.removeView(mBubbleView);
            mBubbleView = null;
            mBubbleIcon = null;
        }
        cleanupNative();
    }

    public void onStatusUpdate(String status) {
        mMainHandler.post(() -> {
            if (status != null && status.startsWith("Error")) {
                isRecording = false;
                isProcessing = false;
                updateBubbleState();
                Toast.makeText(this, status, Toast.LENGTH_SHORT).show();
            }
        });
    }

    public void onAudioLevel(float level) {
    }

    public void onTextTranscribed(String text) {
        mMainHandler.post(() -> {
            isProcessing = false;
            updateBubbleState();

            if (text == null || text.trim().isEmpty()) return;

            TranscriptionHistory.get(this).insert(text, TranscriptionHistory.SOURCE_BUBBLE);

            ClipboardManager cm = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
            cm.setPrimaryClip(ClipData.newPlainText("Transcription", text));

            boolean inserted = InsertionAccessibilityService.tryInsert(this, text);
            if (inserted) {
                Toast.makeText(this, R.string.bubble_inserted, Toast.LENGTH_SHORT).show();
            } else {
                Toast.makeText(this, R.string.bubble_copied, Toast.LENGTH_SHORT).show();
            }
        });
    }

    private void createNotificationChannel() {
        NotificationChannel channel = new NotificationChannel(
                CHANNEL_ID, getString(R.string.bubble_channel_name),
                NotificationManager.IMPORTANCE_LOW);
        NotificationManager manager = getSystemService(NotificationManager.class);
        if (manager != null) manager.createNotificationChannel(channel);
    }

    private Notification createNotification() {
        Intent stopIntent = new Intent(this, BubbleService.class);
        stopIntent.setAction(ACTION_HIDE);
        PendingIntent stopPi = PendingIntent.getService(
                this, 1, stopIntent, PendingIntent.FLAG_IMMUTABLE);

        Intent stopRecIntent = new Intent(this, BubbleService.class);
        stopRecIntent.setAction(ACTION_STOP_RECORDING);
        PendingIntent stopRecPi = PendingIntent.getService(
                this, 2, stopRecIntent, PendingIntent.FLAG_IMMUTABLE);

        return new Notification.Builder(this, CHANNEL_ID)
                .setContentTitle(getString(R.string.bubble_notification_title))
                .setContentText(getString(R.string.bubble_notification_text))
                .setSmallIcon(android.R.drawable.ic_btn_speak_now)
                .addAction(new Notification.Action.Builder(
                        null, getString(R.string.bubble_stop_recording), stopRecPi).build())
                .addAction(new Notification.Action.Builder(
                        null, getString(R.string.bubble_hide), stopPi).build())
                .setOngoing(true)
                .build();
    }

    @Override
    public void onDestroy() {
        hideBubble();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private native void initNative(BubbleService service);
    private native void cleanupNative();
    private native void startRecordingNative();
    private native void stopRecordingNative();
}
