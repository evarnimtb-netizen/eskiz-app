package app.eskiz.sharetest;

import android.content.ContentResolver;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.security.MessageDigest;

/** Reads a content URI and logs its MIME type, size and SHA-256 (tag {@code sharetest}). */
final class Report {
    static final String TAG = "sharetest";

    private Report() {}

    /** Read {@code uri}, log one line and keep a copy in {@code keepDir} as {@code <prefix>-<n>}. */
    static void uri(ContentResolver cr, Uri uri, String what, File keepDir, String prefix, int n) {
        try {
            final String type = cr.getType(uri);
            String name = "";
            try (Cursor c = cr.query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                if (c != null && c.moveToFirst()) {
                    name = c.getString(0);
                }
            } catch (RuntimeException e) {
                // no name
            }
            final MessageDigest md = MessageDigest.getInstance("SHA-256");
            long size = 0;
            File copy = null;
            try (InputStream in = cr.openInputStream(uri)) {
                if (in == null) {
                    Log.i(TAG, what + ": unreadable " + uri);
                    return;
                }
                if (keepDir != null) {
                    keepDir.mkdirs();
                    copy = new File(keepDir, prefix + "-" + n);
                }
                try (FileOutputStream out = copy == null ? null : new FileOutputStream(copy)) {
                    final byte[] buf = new byte[1 << 16];
                    int r;
                    while ((r = in.read(buf)) > 0) {
                        md.update(buf, 0, r);
                        size += r;
                        if (out != null) {
                            out.write(buf, 0, r);
                        }
                    }
                }
            }
            final StringBuilder hex = new StringBuilder();
            for (byte b : md.digest()) {
                hex.append(String.format("%02x", b));
            }
            Log.i(TAG, what + ": type=" + type + " size=" + size + " sha256=" + hex + " name=" + name);
        } catch (Exception e) {
            Log.w(TAG, what + ": failed " + e);
        }
    }
}
