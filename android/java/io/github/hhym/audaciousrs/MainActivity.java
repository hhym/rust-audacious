package io.github.hhym.audaciousrs;

import android.app.NativeActivity;
import android.content.Intent;

/**
 * 只是给 NativeActivity 加一个 onActivityResult 转发：
 * Rust 侧（android.rs）用 ACTION_OPEN_DOCUMENT 拉起系统文件选择器，
 * 选完的结果必须回到这里，再交给 native 代码读流落盘。
 */
public class MainActivity extends NativeActivity {
    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        nativeOnActivityResult(requestCode, resultCode, data);
    }

    private static native void nativeOnActivityResult(int requestCode, int resultCode, Intent data);
}
