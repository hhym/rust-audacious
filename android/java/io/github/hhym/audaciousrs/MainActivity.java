package io.github.hhym.audaciousrs;

import android.app.NativeActivity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.graphics.drawable.Icon;
import android.os.Build;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.media.AudioManager;

/**
 * NativeActivity 的两处 Java 胶水：
 *
 * 1. onActivityResult：Rust 用 ACTION_OPEN_DOCUMENT 拉起系统文件选择器，
 *    选完的结果回到这里，再交给 native 代码（读流落盘）。
 * 2. 音频焦点：播放时申请 AUDIOFOCUS_GAIN；来电/其他 App 抢占时把焦点变化
 *    转给 native，由引擎暂停/恢复。
 */
public class MainActivity extends NativeActivity {
    private AudioManager audioManager;
    private AudioFocusRequest focusRequest;
    private AudioManager.OnAudioFocusChangeListener focusListener;
    private MediaSession mediaSession;
    private NotificationManager notifications;
    private static final String CHANNEL = "playback";
    private static final int NOTIFY_ID = 1001;
    /** 通知栏按钮的动作码，和 Rust 侧 nativeOnNotificationAction 对应。 */
    private static final int ACT_TOGGLE = 1;
    private static final int ACT_NEXT = 2;
    private static final int ACT_PREV = 3;
    private static final int ACT_PLAY = 4;
    private static final int ACT_PAUSE = 5;
    private static final int ACT_STOP = 6;

    @Override
    protected void onCreate(android.os.Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        try {
            notifications = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                NotificationChannel channel = new NotificationChannel(
                        CHANNEL, "播放控制", NotificationManager.IMPORTANCE_LOW);
                channel.setShowBadge(false);
                notifications.createNotificationChannel(channel);
            }
            if (Build.VERSION.SDK_INT >= 33) {
                requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"}, 100);
            }
            // MediaSession：锁屏 / 耳机线控 / 蓝牙按键统一走这里
            mediaSession = new MediaSession(this, "audacious-rs");
            mediaSession.setCallback(new MediaSession.Callback() {
                @Override public void onPlay() { nativeOnMediaAction(ACT_PLAY); }
                @Override public void onPause() { nativeOnMediaAction(ACT_PAUSE); }
                @Override public void onStop() { nativeOnMediaAction(ACT_STOP); }
                @Override public void onSkipToNext() { nativeOnMediaAction(ACT_NEXT); }
                @Override public void onSkipToPrevious() { nativeOnMediaAction(ACT_PREV); }
                @Override public void onSeekTo(long pos) {
                    nativeOnMediaSeek(pos / 1000.0);
                }
            });
            mediaSession.setActive(true);
        } catch (Throwable t) {
            android.util.Log.w("audacious", "初始化通知栏失败", t);
        }
    }

    /** 通知栏按钮回到 Activity（launchMode=singleTop），再转给 native。 */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (intent == null) {
            return;
        }
        int action = intent.getIntExtra("aud_action", 0);
        if (action != 0) {
            nativeOnNotificationAction(action);
        }
    }

    private PendingIntent actionIntent(int action, int requestCode) {
        Intent intent = new Intent(this, MainActivity.class);
        intent.setAction("io.github.hhym.audaciousrs.ACTION");
        intent.putExtra("aud_action", action);
        intent.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        return PendingIntent.getActivity(
                this,
                requestCode,
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    /** Rust 侧每换曲/切状态调一次：更新「正在播放」通知和它的三个按钮。 */
    public void updateNotification(String title, String artist, boolean playing) {
        try {
            if (notifications == null) {
                notifications = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
            }
            Intent open = new Intent(this, MainActivity.class);
            open.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
            PendingIntent content = PendingIntent.getActivity(
                    this, 0, open,
                    PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
            String text = (artist == null || artist.isEmpty()) ? (title == null ? "" : title)
                    : artist + " · " + (title == null ? "" : title);
            Notification.Builder builder = new Notification.Builder(this, CHANNEL)
                    .setSmallIcon(android.R.drawable.ic_media_play)
                    .setContentTitle(title == null ? "" : title)
                    .setContentText(text)
                    .setContentIntent(content)
                    .setOnlyAlertOnce(true)
                    .setOngoing(playing)
                    .setShowWhen(false)
                    .setVisibility(Notification.VISIBILITY_PUBLIC);
            builder.addAction(new Notification.Action.Builder(
                    Icon.createWithResource(this, android.R.drawable.ic_media_previous),
                    "上一首", actionIntent(ACT_PREV, 1)).build());
            builder.addAction(new Notification.Action.Builder(
                    Icon.createWithResource(this,
                            playing ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play),
                    playing ? "暂停" : "播放", actionIntent(ACT_TOGGLE, 2)).build());
            builder.addAction(new Notification.Action.Builder(
                    Icon.createWithResource(this, android.R.drawable.ic_media_next),
                    "下一首", actionIntent(ACT_NEXT, 3)).build());
            if (mediaSession != null) {
                Notification.MediaStyle style = new Notification.MediaStyle()
                        .setMediaSession(mediaSession.getSessionToken())
                        .setShowActionsInCompactView(0, 1, 2);
                builder.setStyle(style);
            }
            notifications.notify(NOTIFY_ID, builder.build());
        } catch (Throwable t) {
            android.util.Log.w("audacious", "更新通知失败", t);
        }
    }

    /** Rust 侧每换曲调一次：把标题/艺人/时长和播放位置写进 MediaSession。 */
    public void updateMediaSession(String title, String artist, boolean playing,
                                   long positionMs, long durationMs) {
        try {
            if (mediaSession == null) {
                return;
            }
            MediaMetadata.Builder meta = new MediaMetadata.Builder()
                    .putString(MediaMetadata.METADATA_KEY_TITLE, title == null ? "" : title)
                    .putString(MediaMetadata.METADATA_KEY_ARTIST, artist == null ? "" : artist)
                    .putLong(MediaMetadata.METADATA_KEY_DURATION, durationMs);
            mediaSession.setMetadata(meta.build());
            PlaybackState.Builder state = new PlaybackState.Builder()
                    .setActions(PlaybackState.ACTION_PLAY
                            | PlaybackState.ACTION_PAUSE
                            | PlaybackState.ACTION_PLAY_PAUSE
                            | PlaybackState.ACTION_STOP
                            | PlaybackState.ACTION_SKIP_TO_NEXT
                            | PlaybackState.ACTION_SKIP_TO_PREVIOUS
                            | PlaybackState.ACTION_SEEK_TO)
                    .setState(playing ? PlaybackState.STATE_PLAYING : PlaybackState.STATE_PAUSED,
                            positionMs, playing ? 1.0f : 0.0f);
            mediaSession.setPlaybackState(state.build());
        } catch (Throwable t) {
            android.util.Log.w("audacious", "更新 MediaSession 失败", t);
        }
    }

    /** 停止播放时把通知收掉。 */
    public void clearNotification() {
        try {
            if (notifications != null) {
                notifications.cancel(NOTIFY_ID);
            }
        } catch (Throwable t) {
            android.util.Log.w("audacious", "清除通知失败", t);
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        nativeOnActivityResult(requestCode, resultCode, data);
    }

    /** Rust 侧在开始播放时调用。 */
    public void requestAudioFocus() {
        try {
            if (audioManager == null) {
                audioManager = (AudioManager) getSystemService(Context.AUDIO_SERVICE);
            }
            if (focusListener == null) {
                focusListener = new AudioManager.OnAudioFocusChangeListener() {
                    @Override
                    public void onAudioFocusChange(int focusChange) {
                        nativeOnAudioFocusChange(focusChange);
                    }
                };
            }
            if (focusRequest == null) {
                AudioAttributes attrs = new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build();
                focusRequest = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                        .setAudioAttributes(attrs)
                        .setOnAudioFocusChangeListener(focusListener)
                        .build();
            }
            audioManager.requestAudioFocus(focusRequest);
        } catch (Throwable t) {
            android.util.Log.w("audacious", "requestAudioFocus 失败", t);
        }
    }

    /** Rust 侧在停止播放时调用。 */
    public void abandonAudioFocus() {
        try {
            if (audioManager != null && focusRequest != null) {
                audioManager.abandonAudioFocusRequest(focusRequest);
            }
        } catch (Throwable t) {
            android.util.Log.w("audacious", "abandonAudioFocus 失败", t);
        }
    }

    private static native void nativeOnActivityResult(int requestCode, int resultCode, Intent data);

    private static native void nativeOnAudioFocusChange(int focusChange);

    private static native void nativeOnNotificationAction(int action);

    private static native void nativeOnMediaAction(int action);

    private static native void nativeOnMediaSeek(double seconds);
}
