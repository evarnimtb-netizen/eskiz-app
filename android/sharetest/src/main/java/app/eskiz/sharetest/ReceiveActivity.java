package app.eskiz.sharetest;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;

import java.util.ArrayList;

/** The share target: logs the MIME type, size and SHA-256 of each stream and keeps a copy. */
public class ReceiveActivity extends Activity {
    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        final Intent in = getIntent();
        final ArrayList<Uri> uris = new ArrayList<>();
        if (Intent.ACTION_SEND.equals(in.getAction())) {
            final Uri u = in.getParcelableExtra(Intent.EXTRA_STREAM);
            if (u != null) {
                uris.add(u);
            }
        } else if (Intent.ACTION_SEND_MULTIPLE.equals(in.getAction())) {
            final ArrayList<Uri> list = in.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (list != null) {
                uris.addAll(list);
            }
        }
        Log.i(Report.TAG, "received: action=" + in.getAction() + " type=" + in.getType() + " streams=" + uris.size());
        int n = 0;
        for (Uri u : uris) {
            Report.uri(getContentResolver(), u, "received", getExternalFilesDir(null), "received", n++);
        }
        finish();
    }
}
