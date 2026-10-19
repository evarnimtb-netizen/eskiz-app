package app.eskiz.sharetest;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.util.Log;

/** Logs the primary clip. Reading the clipboard needs focus on Android 10 and later. */
public class ClipReadActivity extends Activity {
    private boolean done;

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus || done) {
            return;
        }
        done = true;
        final ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        final ClipData clip = cm.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            Log.i(Report.TAG, "clip read: empty");
        } else {
            for (int i = 0; i < clip.getItemCount(); i++) {
                if (clip.getItemAt(i).getUri() != null) {
                    Report.uri(getContentResolver(), clip.getItemAt(i).getUri(), "clip read", getExternalFilesDir(null), "clip", i);
                } else {
                    Log.i(Report.TAG, "clip read: text " + clip.getItemAt(i).getText());
                }
            }
        }
        finish();
    }
}
