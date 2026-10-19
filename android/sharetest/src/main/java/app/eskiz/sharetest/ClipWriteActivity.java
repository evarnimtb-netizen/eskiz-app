package app.eskiz.sharetest;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.net.Uri;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;

/** Puts a known 64 x 64 PNG (a solid colour drawn in code) on the clipboard. */
public class ClipWriteActivity extends Activity {
    /** The test colour: the script checks the pasted pixel against it. */
    static final int COLOUR = Color.rgb(32, 128, 255);
    private boolean done;

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus || done) {
            return;
        }
        done = true;
        try {
            final Bitmap bmp = Bitmap.createBitmap(64, 64, Bitmap.Config.ARGB_8888);
            bmp.eraseColor(COLOUR);
            final File file = new File(getCacheDir(), "clip.png");
            try (FileOutputStream out = new FileOutputStream(file)) {
                bmp.compress(Bitmap.CompressFormat.PNG, 100, out);
            }
            final Uri uri = new Uri.Builder().scheme("content").authority(TestProvider.AUTHORITY).appendPath("clip.png").build();
            final ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            cm.setPrimaryClip(ClipData.newUri(getContentResolver(), "Share test image", uri));
            Log.i(Report.TAG, "clip written: colour=#" + String.format("%06x", COLOUR & 0xFFFFFF) + " size=64x64");
        } catch (Exception e) {
            Log.w(Report.TAG, "clip write failed: " + e);
        }
        finish();
    }
}
