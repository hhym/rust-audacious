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
    static {
        // NativeActivity 是框架直接 dlopen 的，ART 不认这个库的 JNI 符号；
        // 再用 System.loadLibrary 注册一遍，native 方法才能解析到。
        System.loadLibrary("audacious_rs");
    }

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

    /** 当前 Activity 实例，给 native 线程（egui 渲染线程）回主线程用。 */
    private static MainActivity sInstance;

    /**
     * 从任意线程拉起系统文件选择器：切到 UI 线程再 startActivityForResult，
     * 并补上 ACTION_OPEN_DOCUMENT 必需的 CATEGORY_OPENABLE 与读权限 flag。
     */
    public static boolean openPicker(final int requestCode, final String mime, final boolean multiple) {
        final MainActivity self = sInstance;
        if (self == null) {
            android.util.Log.w("AudaciousRS", "openPicker: 没有 Activity 实例");
            return false;
        }
        // 精简 ROM 上可能压根没有 DocumentsUI，先探测一下，好让 native 侧退回扫描。
        try {
            Intent probe = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            probe.addCategory(Intent.CATEGORY_OPENABLE);
            probe.setType(mime);
            if (probe.resolveActivity(self.getPackageManager()) == null) {
                android.util.Log.w("AudaciousRS", "本机没有文档选择器，改用媒体库扫描");
                return false;
            }
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "探测选择器失败: " + t);
            return false;
        }
        self.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                    i.addCategory(Intent.CATEGORY_OPENABLE);
                    i.setType(mime);
                    i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                            | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
                    if (multiple) {
                        i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                    }
                    self.startActivityForResult(i, requestCode);
                } catch (Throwable t) {
                    android.util.Log.e("AudaciousRS", "openPicker 失败: " + t);
                }
            }
        });
        return true;
    }

    /** 插件选择：mode 0 = 选 .zip 压缩包，mode 1 = 选插件目录。 */
    public static boolean openPluginPick(final int mode) {
        final MainActivity self = sInstance;
        if (self == null) {
            return false;
        }
        try {
            Intent probe;
            if (mode == 0) {
                probe = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                probe.addCategory(Intent.CATEGORY_OPENABLE);
                probe.setType("application/zip");
            } else {
                probe = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            }
            if (probe.resolveActivity(self.getPackageManager()) == null) {
                android.util.Log.w("AudaciousRS", "本机没有插件选择器，退回内置浏览器");
                return false;
            }
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "探测插件选择器失败: " + t);
            return false;
        }
        self.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    Intent i;
                    if (mode == 0) {
                        i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                        i.addCategory(Intent.CATEGORY_OPENABLE);
                        i.setType("application/zip");
                        i.putExtra(Intent.EXTRA_MIME_TYPES,
                                new String[]{ "application/zip", "application/x-zip-compressed" });
                    } else {
                        i = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
                    }
                    i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                    self.startActivityForResult(i, mode == 0 ? 7003 : 7004);
                } catch (Throwable t) {
                    android.util.Log.e("AudaciousRS", "插件选择器拉起失败: " + t);
                }
            }
        });
        return true;
    }

    /** SAF 的 content:// uri 读流拷成真实文件，给 native 侧当路径用。 */
    private static String copyUriToCache(android.net.Uri uri, String name) {
        final MainActivity self = sInstance;
        if (self == null || uri == null) {
            return null;
        }
        java.io.InputStream in = null;
        java.io.FileOutputStream out = null;
        try {
            in = self.getContentResolver().openInputStream(uri);
            if (in == null) {
                return null;
            }
            java.io.File f = new java.io.File(self.getCacheDir(), name);
            out = new java.io.FileOutputStream(f);
            byte[] buf = new byte[65536];
            int n;
            while ((n = in.read(buf)) > 0) {
                out.write(buf, 0, n);
            }
            out.flush();
            return f.getAbsolutePath();
        } catch (Throwable t) {
            android.util.Log.e("AudaciousRS", "拷贝选择结果失败: " + t);
            return null;
        } finally {
            try { if (out != null) out.close(); } catch (Throwable ignored) {}
            try { if (in != null) in.close(); } catch (Throwable ignored) {}
        }
    }

    /** SAF 的 tree uri（content://…/tree/primary%3ADownload%2Fxx）还原成真实路径。 */
    private static String treeUriToPath(android.net.Uri uri) {
        try {
            String id = android.provider.DocumentsContract.getTreeDocumentId(uri);
            if (id == null) {
                return null;
            }
            int colon = id.indexOf(':');
            String vol = colon > 0 ? id.substring(0, colon) : "primary";
            String rel = colon > 0 ? id.substring(colon + 1) : id;
            String base = "primary".equals(vol) ? "/storage/emulated/0/" : "/storage/" + vol + "/";
            return base + rel;
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "tree uri 转路径失败: " + t);
            return null;
        }
    }

    /** 这台机器上没有 DocumentsUI（SAF 选择器）时，改用 MediaStore 扫描本地音乐。 */
    public static String[] scanAudioPaths() {
        final MainActivity self = sInstance;
        if (self == null) {
            return new String[0];
        }
        java.util.ArrayList<String> out = new java.util.ArrayList<String>();
        try {
            android.database.Cursor c = self.getContentResolver().query(
                    android.provider.MediaStore.Audio.Media.EXTERNAL_CONTENT_URI,
                    new String[]{ android.provider.MediaStore.Audio.Media.DATA },
                    android.provider.MediaStore.Audio.Media.IS_MUSIC + "!=0",
                    null, null);
            if (c != null) {
                while (c.moveToNext()) {
                    String p = c.getString(0);
                    if (p != null && p.length() > 0) {
                        out.add(p);
                    }
                }
                c.close();
            }
        } catch (Throwable t) {
            android.util.Log.e("AudaciousRS", "扫描媒体库失败: " + t);
        }
        return out.toArray(new String[0]);
    }

    /** 8.x / 9.x 需要读外部存储的运行时权限，没有它 MediaStore 查不到东西。 */
    public static void ensureStoragePermission() {
        final MainActivity self = sInstance;
        if (self == null || Build.VERSION.SDK_INT >= 33) {
            return;
        }
        try {
            if (self.checkSelfPermission(android.Manifest.permission.READ_EXTERNAL_STORAGE)
                    != android.content.pm.PackageManager.PERMISSION_GRANTED) {
                self.requestPermissions(
                        new String[]{ android.Manifest.permission.READ_EXTERNAL_STORAGE }, 7001);
            }
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "申请存储权限失败: " + t);
        }
    }

    /** 状态栏 / 导航栏占位（像素），由原生 UI 用来给内容让位。 */
    private static int insetTop = 0;
    private static int insetBottom = 0;

    public static int getInsetTopPx() {
        return insetTop;
    }

    public static int getInsetBottomPx() {
        return insetBottom;
    }

    /** 监听系统窗口内边距变化，把状态栏/导航栏高度转给原生层。 */
    private void watchInsets() {
        try {
            final android.view.View decor = getWindow().getDecorView();
            decor.setOnApplyWindowInsetsListener(new android.view.View.OnApplyWindowInsetsListener() {
                @Override
                public android.view.WindowInsets onApplyWindowInsets(android.view.View v, android.view.WindowInsets insets) {
                    int top = 0;
                    int bottom = 0;
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                        top = insets.getSystemWindowInsetTop();
                        bottom = insets.getSystemWindowInsetBottom();
                    }
                    if (top != insetTop || bottom != insetBottom) {
                        insetTop = top;
                        insetBottom = bottom;
                        android.util.Log.i("AudaciousRS", "insets top=" + top + " bottom=" + bottom
                                + " h=" + v.getHeight() + " w=" + v.getWidth());
                        nativeSetInsets(top, bottom);
                    }
                    return insets;
                }
            });
            decor.requestApplyInsets();
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "读取窗口内边距失败: " + t);
        }
    }

    /**
     * 沉浸式全屏：状态栏/导航栏收起，整个屏幕都归应用。
     * 不这么做的话，横屏时状态栏会盖在最上面 27pt 上，那一条的点击会被 SystemUI 吃掉，
     * 看起来就是「菜单栏点了没反应」，下半屏也会因为 insets 计算错位点不动。
     */
    private void goImmersive() {
        try {
            // 窗口级全屏标记：隐藏状态栏，避免它盖在菜单栏上吃掉那一条的点击
            getWindow().addFlags(android.view.WindowManager.LayoutParams.FLAG_FULLSCREEN);
            android.view.View decor = getWindow().getDecorView();
            decor.setSystemUiVisibility(
                    android.view.View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                            | android.view.View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                            | android.view.View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                            | android.view.View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                            | android.view.View.SYSTEM_UI_FLAG_FULLSCREEN
                            | android.view.View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "设置沉浸式全屏失败: " + t);
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            goImmersive();
        }
    }

    @Override
    protected void onCreate(android.os.Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        sInstance = this;
        setupImeBridge();
        goImmersive();
        // eframe/winit 在初始化时可能覆写 systemUiVisibility，隔一会儿再补一次
        new android.os.Handler(android.os.Looper.getMainLooper())
                .postDelayed(new Runnable() { public void run() { goImmersive(); } }, 800);
        watchInsets();
        ensureStoragePermission();
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
        if (requestCode == 7003 || requestCode == 7004) {
            if (resultCode == RESULT_OK && data != null && data.getData() != null) {
                final android.net.Uri uri = data.getData();
                if (requestCode == 7003) {
                    // 压缩包可能几十 MB，放到后台线程拷
                    new Thread(new Runnable() {
                        @Override
                        public void run() {
                            String path = copyUriToCache(uri, "plugin-pick.zip");
                            if (path != null) {
                                nativePluginReady(path);
                            } else {
                                android.util.Log.e("AudaciousRS", "插件压缩包拷贝失败");
                            }
                        }
                    }).start();
                } else {
                    String path = treeUriToPath(uri);
                    if (path != null) {
                        nativePluginReady(path);
                    }
                }
            }
            return;
        }
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

    // ------------------------------------------------------------------ 软键盘
    // NativeActivity 的 SurfaceView 没有 InputConnection：系统软键盘能弹出来，
    // 但敲的字没有接收方，表现就是「搜索框输入不了」。这里挂一个 1px 隐形
    // EditText 接住输入，再把内容差异转成 nativeImeText / nativeImeKey 交给 Rust。
    private android.widget.EditText imeBox;
    private String imeLast = "";
    private boolean imeEnglish = true;

    private int imeInputType(boolean english) {
        return android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                | (english ? android.text.InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD : 0);
    }

    private void setupImeBridge() {
        try {
            imeBox = new android.widget.EditText(this);
            imeBox.setBackgroundColor(0x00000000);
            imeBox.setTextColor(0x00000000);
            imeBox.setAlpha(0f);
            imeBox.setCursorVisible(false);
            imeBox.setSingleLine(true);
            // 可见密码变体：输入法不走拼音组合态，英文/数字按下即提交，
            // 搜索框逐字回显；中文仍可切到中文输入法后选字提交
            imeBox.setInputType(imeInputType(imeEnglish));
            imeBox.setImeOptions(android.view.inputmethod.EditorInfo.IME_FLAG_NO_EXTRACT_UI
                    | android.view.inputmethod.EditorInfo.IME_FLAG_NO_FULLSCREEN);
            getWindow().addContentView(imeBox, new android.view.ViewGroup.LayoutParams(1, 1));
            imeBox.setVisibility(android.view.View.INVISIBLE);
            android.util.Log.i("AudaciousRS", "软键盘桥接已挂上");
            imeBox.addTextChangedListener(new android.text.TextWatcher() {
                @Override
                public void beforeTextChanged(CharSequence s, int start, int count, int after) {
                }

                @Override
                public void onTextChanged(CharSequence s, int start, int before, int count) {
                }

                @Override
                public void afterTextChanged(android.text.Editable s) {
                    forwardIme(s);
                }
            });
            imeBox.setOnEditorActionListener(new android.widget.TextView.OnEditorActionListener() {
                @Override
                public boolean onEditorAction(android.widget.TextView v, int actionId, android.view.KeyEvent event) {
                    if (event == null) {
                        nativeImeKey(13);
                        return true;
                    }
                    return false;
                }
            });
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "初始化软键盘桥接失败: " + t);
        }
    }

    /** 由原生层调用：egui 的文本框拿到/失去焦点时抬起或收起软键盘。 */
    public void imeShow(final boolean show) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (imeBox == null) {
                    return;
                }
                try {
                    android.view.inputmethod.InputMethodManager imm =
                            (android.view.inputmethod.InputMethodManager)
                                    getSystemService(INPUT_METHOD_SERVICE);
                    if (show) {
                        imeBox.setVisibility(android.view.View.VISIBLE);
                        imeBox.setFocusable(true);
                        imeBox.setFocusableInTouchMode(true);
                        imeBox.requestFocus();
                        // 表面视图有时把焦点抢回去，隔一会儿确认一次并重试
                        imeBox.postDelayed(new Runnable() {
                            @Override
                            public void run() {
                                if (imeBox == null || imeBox.isFocused()) {
                                    return;
                                }
                                imeBox.requestFocus();
                                android.view.inputmethod.InputMethodManager m2 =
                                        (android.view.inputmethod.InputMethodManager)
                                                getSystemService(INPUT_METHOD_SERVICE);
                                if (m2 != null) {
                                    m2.showSoftInput(imeBox,
                                            android.view.inputmethod.InputMethodManager.SHOW_IMPLICIT);
                                }
                            }
                        }, 400);
                        if (imm != null) {
                            imm.showSoftInput(imeBox,
                                    android.view.inputmethod.InputMethodManager.SHOW_IMPLICIT);
                        }
                    } else {
                        imeLast = "";
                        imeBox.setText("");
                        imeBox.clearFocus();
                        if (imm != null) {
                            imm.hideSoftInputFromWindow(imeBox.getWindowToken(), 0);
                        }
                        imeBox.setVisibility(android.view.View.INVISIBLE);
                    }
                } catch (Throwable t) {
                    android.util.Log.w("AudaciousRS", "切换软键盘失败: " + t);
                }
            }
        });
    }

    /** 由原生层调用：切换英文直输 / 中文（拼音选用）两种软键盘模式。 */
    public void imeSetEnglish(final boolean english) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (imeBox == null) {
                    return;
                }
                try {
                    boolean up = imeBox.getVisibility() == android.view.View.VISIBLE
                            && imeBox.isFocused();
                    imeEnglish = english;
                    imeLast = "";
                    imeBox.setInputType(imeInputType(english));
                    imeBox.setText("");
                    if (up) {
                        final android.view.inputmethod.InputMethodManager imm =
                                (android.view.inputmethod.InputMethodManager)
                                        getSystemService(INPUT_METHOD_SERVICE);
                        if (imm != null) {
                            // 让输入法按新的 inputType 重建连接
                            imm.restartInput(imeBox);
                        }
                        // 有的输入法 restartInput 后会把键盘收回去，补一次重新抬起
                        imeBox.postDelayed(new Runnable() {
                            @Override
                            public void run() {
                                if (imeBox == null) {
                                    return;
                                }
                                if (!imeBox.isFocused()) {
                                    imeBox.requestFocus();
                                }
                                android.view.inputmethod.InputMethodManager m2 =
                                        (android.view.inputmethod.InputMethodManager)
                                                getSystemService(INPUT_METHOD_SERVICE);
                                if (m2 != null) {
                                    m2.showSoftInput(imeBox,
                                            android.view.inputmethod.InputMethodManager.SHOW_IMPLICIT);
                                }
                            }
                        }, 300);
                    }
                } catch (Throwable t) {
                    android.util.Log.w("AudaciousRS", "切换输入模式失败: " + t);
                }
            }
        });
    }

    /** 只把「和上一次已提交内容相比多/少了哪些」转给原生层。 */
    private void forwardIme(android.text.Editable s) {
        if (imeBox == null) {
            return;
        }
        String now = s.toString();
        if (now.equals(imeLast)) {
            return;
        }
        String prev = imeLast;
        int head = 0;
        int maxHead = Math.min(prev.length(), now.length());
        while (head < maxHead && prev.charAt(head) == now.charAt(head)) {
            head++;
        }
        int tail = 0;
        int maxTail = Math.min(prev.length() - head, now.length() - head);
        while (tail < maxTail
                && prev.charAt(prev.length() - 1 - tail) == now.charAt(now.length() - 1 - tail)) {
            tail++;
        }
        int dels = prev.length() - head - tail;
        imeLast = now;
        for (int i = 0; i < dels; i++) {
            nativeImeKey(8);
        }
        String inserted = now.substring(head, now.length() - tail);
        if (!inserted.isEmpty()) {
            nativeImeText(inserted);
        }
    }

    /**
     * 软键盘的退格/回车走的是按键事件，按键被派发到窗口最外层（NativeView）落不到
     * 隐形 EditText 上，所以在这里统一拦一下，转成和输入法提交同样的增量。
     */
    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        try {
            boolean imeUp = imeBox != null
                    && imeBox.getVisibility() == android.view.View.VISIBLE
                    && imeBox.isFocused();
            if (imeUp && event != null) {
                int code = event.getKeyCode();
                boolean isDel = code == android.view.KeyEvent.KEYCODE_DEL;
                boolean isEnter = code == android.view.KeyEvent.KEYCODE_ENTER
                        || code == android.view.KeyEvent.KEYCODE_NUMPAD_ENTER;
                if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
                    if (isDel) {
                        android.text.Editable e = imeBox.getText();
                        if (e != null && e.length() > 0) {
                            e.delete(e.length() - 1, e.length());
                        }
                        return true;
                    }
                    if (isEnter) {
                        nativeImeKey(13);
                        return true;
                    }
                } else if (event.getAction() == android.view.KeyEvent.ACTION_UP) {
                    if (isDel || isEnter) {
                        return true;
                    }
                }
            }
        } catch (Throwable t) {
            android.util.Log.w("AudaciousRS", "拦截按键失败: " + t);
        }
        return super.dispatchKeyEvent(event);
    }

    private static native void nativeOnActivityResult(int requestCode, int resultCode, Intent data);

    private static native void nativeOnAudioFocusChange(int focusChange);

    private static native void nativeOnNotificationAction(int action);

    private static native void nativeOnMediaAction(int action);

    private static native void nativeOnMediaSeek(double seconds);

    /** 状态栏/导航栏内边距上报（像素）。 */
    private static native void nativeSetInsets(int top, int bottom);

    /** 软键盘敲进来的文字。 */
    private static native void nativeImeText(String text);

    /** 软键盘的特殊键：8 退格 / 13 回车。 */
    private static native void nativeImeKey(int code);
    /** 插件选择结果（zip 缓存路径或插件目录）回传。 */
    private static native void nativePluginReady(String path);
}
