package app.eskiz;

import android.os.Bundle;
import android.view.KeyEvent;
import android.view.MotionEvent;

import android.app.NativeActivity;

/**
 * The app's single activity: a NativeActivity whose behaviour lives in the Rust library
 * (libeskiz_app.so, entry point android_main).
 *
 * <h3>Input (A2)</h3>
 * NativeActivity hands the window's input queue to native code, so {@code dispatchTouchEvent}
 * never fires and winit only sees position and pressure for every tool type (no stylus, tilt,
 * orientation, cancel flag or batched samples). This class takes the queue back
 * ({@code takeInputQueue(null)}, before the window is attached) and forwards every touch,
 * hover, scroll and key event over JNI to the Rust side (src/android_queue.rs), which turns
 * them into pen events and egui events (src/android_input.rs).
 *
 * Kill switch: launch with {@code --ez native_input true}
 * ({@code adb shell am start -n app.eskiz/.EskizActivity --ez native_input true}) to keep the
 * system's native input queue instead (winit touch only, no stylus data).
 */
public class EskizActivity extends NativeActivity {
    static {
        // Same library NativeActivity loads (meta-data android.app.lib_name); loading it here
        // as well binds the native* methods below and returns the existing handle.
        System.loadLibrary("eskiz_app");
    }

    private boolean javaInput = true;

    // Implemented in Rust (android_queue.rs, jni_exports). Primitives only.
    private static native void nativeBegin(int action, int actionIndex, int flags, int buttons,
            int pointerCount, float hscroll, float vscroll);

    private static native void nativeSample(long timeMs, int id, int tool, float x, float y,
            float pressure, float tilt, float orientation);

    private static native void nativeEnd();

    /** The activity is stopped (true) or started again (false): real suspend vs periodic save. */
    private static native void nativeBackground(boolean stopped);

    /** System bar and display cutout insets in pixels (edge-to-edge, targetSdk 36). */
    private static native void nativeInsets(int left, int top, int right, int bottom);

    private static native void nativeKey(boolean pressed, boolean repeat, int keyCode, int unicode,
            int meta);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        javaInput = !getIntent().getBooleanExtra("native_input", false);
        if (javaInput) {
            // NativeActivity.onCreate registered itself as the input queue callback. The window
            // reads that callback when it is attached, which is after this method.
            getWindow().takeInputQueue(null);
        }
        final android.view.View decor = getWindow().getDecorView();
        decor.setOnApplyWindowInsetsListener((v, insets) -> {
            reportInsets(insets);
            return v.onApplyWindowInsets(insets);
        });
        decor.requestApplyInsets();
    }

    /** Status bar, navigation bar and display cutout: the largest of each side. */
    private static void reportInsets(android.view.WindowInsets w) {
        int l;
        int t;
        int r;
        int b;
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            final android.graphics.Insets i = w.getInsets(android.view.WindowInsets.Type.systemBars()
                    | android.view.WindowInsets.Type.displayCutout());
            l = i.left;
            t = i.top;
            r = i.right;
            b = i.bottom;
        } else {
            l = w.getSystemWindowInsetLeft();
            t = w.getSystemWindowInsetTop();
            r = w.getSystemWindowInsetRight();
            b = w.getSystemWindowInsetBottom();
            final android.view.DisplayCutout c = w.getDisplayCutout();
            if (c != null) {
                l = Math.max(l, c.getSafeInsetLeft());
                t = Math.max(t, c.getSafeInsetTop());
                r = Math.max(r, c.getSafeInsetRight());
                b = Math.max(b, c.getSafeInsetBottom());
            }
        }
        nativeInsets(l, t, r, b);
    }

    @Override
    protected void onStop() {
        // Before super: NativeActivity's stop and window teardown follow, and eframe saves then.
        nativeBackground(true);
        super.onStop();
    }

    @Override
    protected void onStart() {
        nativeBackground(false);
        super.onStart();
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent e) {
        if (javaInput) {
            forward(e);
            return true;
        }
        return super.dispatchTouchEvent(e);
    }

    /** Stylus and mouse hover, mouse wheel and button events arrive here, not as touch. */
    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent e) {
        if (javaInput && (e.getSource() & android.view.InputDevice.SOURCE_CLASS_POINTER) != 0) {
            forward(e);
            return true;
        }
        return super.dispatchGenericMotionEvent(e);
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent e) {
        if (javaInput) {
            switch (e.getKeyCode()) {
                case KeyEvent.KEYCODE_BACK:
                case KeyEvent.KEYCODE_VOLUME_UP:
                case KeyEvent.KEYCODE_VOLUME_DOWN:
                case KeyEvent.KEYCODE_VOLUME_MUTE:
                case KeyEvent.KEYCODE_POWER:
                case KeyEvent.KEYCODE_CAMERA:
                    break; // the system's own handling
                default:
                    nativeKey(e.getAction() == KeyEvent.ACTION_DOWN, e.getRepeatCount() > 0,
                            e.getKeyCode(), e.getUnicodeChar(e.getMetaState()), e.getMetaState());
                    return true;
            }
        }
        return super.dispatchKeyEvent(e);
    }

    /** One event: header, then every pointer of every batched sample (oldest first), then the event's own. */
    private static void forward(MotionEvent e) {
        final int n = e.getPointerCount();
        final int history = e.getHistorySize();
        nativeBegin(e.getActionMasked(), e.getActionIndex(), e.getFlags(), e.getButtonState(), n,
                e.getAxisValue(MotionEvent.AXIS_HSCROLL), e.getAxisValue(MotionEvent.AXIS_VSCROLL));
        for (int h = 0; h < history; h++) {
            final long t = e.getHistoricalEventTime(h);
            for (int p = 0; p < n; p++) {
                nativeSample(t, e.getPointerId(p), e.getToolType(p), e.getHistoricalX(p, h),
                        e.getHistoricalY(p, h), e.getHistoricalPressure(p, h),
                        e.getHistoricalAxisValue(MotionEvent.AXIS_TILT, p, h),
                        e.getHistoricalAxisValue(MotionEvent.AXIS_ORIENTATION, p, h));
            }
        }
        final long t = e.getEventTime();
        for (int p = 0; p < n; p++) {
            nativeSample(t, e.getPointerId(p), e.getToolType(p), e.getX(p), e.getY(p),
                    e.getPressure(p), e.getAxisValue(MotionEvent.AXIS_TILT, p),
                    e.getAxisValue(MotionEvent.AXIS_ORIENTATION, p));
        }
        nativeEnd();
    }
}
