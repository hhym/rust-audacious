package io.github.hhym.audaciousrs;

import android.app.NativeActivity;
import android.content.Context;
import android.content.Intent;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
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
}
