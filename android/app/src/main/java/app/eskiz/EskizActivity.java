package app.eskiz;

import android.app.NativeActivity;
import android.content.ClipData;
import android.content.ClipDescription;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.UriPermission;
import android.content.pm.ApplicationInfo;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.util.Log;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.concurrent.ConcurrentHashMap;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

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
 * <h3>Files, share sheet, clipboard and Back (A4)</h3>
 * The Storage Access Framework pickers, the copies to and from the app cache, the share sheet,
 * the system clipboard for images and the Back callback live below. Rust calls the methods in
 * the "A4" section (through the jni crate, see src/android_bridge.rs) and every one returns at
 * once; copies run on one background executor. Results come back through the
 * {@code nativePickResult}, {@code nativeCommitResult} and {@code nativeClipResult} functions with
 * the request id Rust gave. Debug builds (FLAG_DEBUGGABLE only) accept the intent extra
 * {@code eskiz_test} for the emulator script; release builds ignore it.
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

    // A4 natives (src/android_bridge.rs). Strings, so these go through the jni crate.
    private static native void nativePickResult(int req, int status, String path, String name,
            String mime);

    private static native void nativeCommitResult(int req, int status, String text);

    private static native void nativeClipResult(int req, int status, String path, String text);

    /** The system Back was pressed while Rust said it wants it. */
    private static native void nativeBack();

    /** A debug test hook arrived (only called when the APK is debuggable). */
    private static native void nativeTestHook(String hook);

    /** A message for the app to toast. */
    private static native void nativeNotice(String text);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        restoreRequests(savedInstanceState);
        cleanCacheLater();
        handleTestIntent(getIntent());
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

    // ---------------------------------------------------------------------------------------
    // A4: files, share sheet, clipboard, Back
    // ---------------------------------------------------------------------------------------

    private static final String TAG = "eskiz";
    private static final String PREFS = "eskiz-files";
    private static final String PREF_TREE = "export_tree";
    private static final int KIND_OPEN = 1;
    private static final int KIND_CREATE = 2;
    private static final int KIND_TREE = 3;
    private static final String CLIP_LABEL = "Eskiz image #";

    /** One thread for every copy, so a 200 MB file never runs on the UI thread. */
    private final ExecutorService copier = Executors.newSingleThreadExecutor();
    /** Picker kind and staging folder per request id. Saved across process death. */
    private final ConcurrentHashMap<Integer, Integer> kinds = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<Integer, String> dirs = new ConcurrentHashMap<>();
    /** Documents made by create pickers, by request id. */
    private final ConcurrentHashMap<Integer, Uri> docs = new ConcurrentHashMap<>();
    private boolean backWanted = false;
    private boolean backRegistered = false;
    private OnBackInvokedCallback backCallback;

    private boolean debuggable() {
        return (getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0;
    }

    /** The app cache folder, as a path (Rust builds its staging paths under it). */
    public String cacheRoot() {
        return getCacheDir().getAbsolutePath();
    }

    // ---- pickers ----

    /** ACTION_OPEN_DOCUMENT; the copy lands in {@code dir/in}. */
    public void pickOpen(final int req, final String[] mimes, final String dir) {
        runOnUiThread(() -> {
            final Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("*/*");
            if (mimes != null && mimes.length > 0) {
                i.putExtra(Intent.EXTRA_MIME_TYPES, mimes);
            }
            startPicker(req, KIND_OPEN, dir, i);
        });
    }

    /** ACTION_CREATE_DOCUMENT with a suggested name. */
    public void pickCreate(final int req, final String mime, final String name) {
        runOnUiThread(() -> {
            final Intent i = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType(mime == null || mime.isEmpty() ? "application/octet-stream" : mime);
            i.putExtra(Intent.EXTRA_TITLE, name);
            startPicker(req, KIND_CREATE, "", i);
        });
    }

    /** ACTION_OPEN_DOCUMENT_TREE for the export folder. */
    public void pickTree(final int req) {
        runOnUiThread(() -> {
            final Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            startPicker(req, KIND_TREE, "", i);
        });
    }

    @SuppressWarnings("deprecation")
    private void startPicker(int req, int kind, String dir, Intent intent) {
        kinds.put(req, kind);
        dirs.put(req, dir);
        try {
            startActivityForResult(intent, req);
        } catch (RuntimeException e) {
            kinds.remove(req);
            dirs.remove(req);
            nativePickResult(req, 2, "", "No file picker is available on this device.", "");
        }
    }

    @Override
    @SuppressWarnings("deprecation")
    protected void onActivityResult(final int req, final int result, final Intent data) {
        final Integer kind = kinds.remove(req);
        final String dir = dirs.remove(req);
        if (kind == null) {
            // Not a request of this process (it started before the process died): Rust will not
            // know the id either and shows its "interrupted" message.
            nativePickResult(req, result == RESULT_OK ? 0 : 1, "", "", "");
            super.onActivityResult(req, result, data);
            return;
        }
        final Uri uri = data == null ? null : data.getData();
        if (result != RESULT_OK || uri == null) {
            Log.i(TAG, "saf: cancelled req=" + req);
            nativePickResult(req, 1, "", "", "");
            return;
        }
        switch (kind) {
            case KIND_OPEN:
                copier.execute(() -> copyIn(req, uri, dir));
                break;
            case KIND_CREATE:
                docs.put(req, uri);
                nativePickResult(req, 0, "", displayName(uri), mimeOf(uri));
                break;
            default:
                try {
                    persistTree(uri);
                    nativePickResult(req, 0, "", treeName(uri), "");
                } catch (RuntimeException e) {
                    nativePickResult(req, 2, "", "The folder could not be used: " + e.getMessage(), "");
                }
                break;
        }
    }

    private String mimeOf(Uri uri) {
        final String t = getContentResolver().getType(uri);
        return t == null ? "" : t;
    }

    /** The provider's display name for a document URI. */
    private String displayName(Uri uri) {
        try (Cursor c = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME},
                null, null, null)) {
            if (c != null && c.moveToFirst() && !c.isNull(0)) {
                return c.getString(0);
            }
        } catch (RuntimeException e) {
            // fall through
        }
        final String last = uri.getLastPathSegment();
        return last == null ? "" : last;
    }

    /** Copy the picked document into {@code dir/in} (worker thread). */
    private void copyIn(int req, Uri uri, String dir) {
        final ContentResolver cr = getContentResolver();
        final String name = displayName(uri);
        final String mime = mimeOf(uri);
        try {
            final File folder = new File(dir);
            if (!folder.isDirectory() && !folder.mkdirs()) {
                throw new IOException("The cache folder is not available.");
            }
            long size = -1;
            try (Cursor c = cr.query(uri, new String[] {OpenableColumns.SIZE}, null, null, null)) {
                if (c != null && c.moveToFirst() && !c.isNull(0)) {
                    size = c.getLong(0);
                }
            } catch (RuntimeException e) {
                // unknown size
            }
            if (size > 0 && getCacheDir().getUsableSpace() < size + (8L << 20)) {
                nativePickResult(req, 2, "", "Not enough free space to copy the file.", "");
                return;
            }
            final File out = new File(folder, "in");
            final long t0 = System.nanoTime();
            try (InputStream in = cr.openInputStream(uri)) {
                if (in == null) {
                    throw new IOException("The file could not be opened.");
                }
                copyStream(in, new FileOutputStream(out));
            }
            Log.i(TAG, "saf: copied in " + out.length() + " bytes in " + (System.nanoTime() - t0) / 1000000 + " ms");
            Log.i(TAG, "saf: picked " + name);
            nativePickResult(req, 0, out.getAbsolutePath(), name, mime);
        } catch (IOException | RuntimeException e) {
            Log.w(TAG, "saf: copy in failed", e);
            nativePickResult(req, 2, "", "The file could not be read: " + e.getMessage(), "");
        }
    }

    private static void copyStream(InputStream in, OutputStream out) throws IOException {
        try (OutputStream o = out) {
            final byte[] buf = new byte[1 << 20];
            int n;
            while ((n = in.read(buf)) > 0) {
                o.write(buf, 0, n);
            }
        }
    }

    // ---- the export folder (a persisted tree) ----

    private SharedPreferences prefs() {
        return getSharedPreferences(PREFS, MODE_PRIVATE);
    }

    private void persistTree(Uri uri) {
        final ContentResolver cr = getContentResolver();
        cr.takePersistableUriPermission(uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        final String old = prefs().getString(PREF_TREE, "");
        if (!old.isEmpty() && !old.equals(uri.toString())) {
            try {
                cr.releasePersistableUriPermission(Uri.parse(old),
                        Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
            } catch (RuntimeException e) {
                // already gone
            }
        }
        prefs().edit().putString(PREF_TREE, uri.toString()).apply();
        Log.i(TAG, "saf: tree persisted " + treeName(uri));
    }

    /** A readable name for a tree: its last path component, e.g. {@code Documents/Eskiz}. */
    private String treeName(Uri tree) {
        try {
            final String id = DocumentsContract.getTreeDocumentId(tree);
            final int colon = id.indexOf(':');
            final String path = colon >= 0 ? id.substring(colon + 1) : id;
            return path.isEmpty() ? "Internal storage" : path;
        } catch (RuntimeException e) {
            return "Export folder";
        }
    }

    /** The chosen export folder's name while its permission is held, else the empty string. */
    public String treeState() {
        final String s = prefs().getString(PREF_TREE, "");
        if (s.isEmpty()) {
            return "";
        }
        for (UriPermission p : getContentResolver().getPersistedUriPermissions()) {
            if (p.getUri().toString().equals(s) && p.isWritePermission()) {
                // The folder itself may have been deleted: a query on its root tells.
                final Uri tree = Uri.parse(s);
                final Uri doc = DocumentsContract.buildDocumentUriUsingTree(tree,
                        DocumentsContract.getTreeDocumentId(tree));
                try (Cursor c = getContentResolver().query(doc,
                        new String[] {DocumentsContract.Document.COLUMN_DISPLAY_NAME}, null, null, null)) {
                    if (c == null || !c.moveToFirst()) {
                        return "";
                    }
                } catch (RuntimeException e) {
                    return "";
                }
                return treeName(tree);
            }
        }
        return "";
    }

    // ---- commit: copy staged files to where the user chose ----

    /** Copy the staged file to the document made by the create picker {@code docReq}. */
    public void commitFile(final int commitReq, final int docReq, final String staged) {
        copier.execute(() -> {
            final Uri uri = docs.get(docReq);
            if (uri == null) {
                nativeCommitResult(commitReq, 2, "The place to save to is gone. Choose it again.");
                return;
            }
            try {
                final long t0 = System.nanoTime();
                writeTo(uri, new File(staged));
                final String name = displayName(uri);
                Log.i(TAG, "saf: committed " + name + " in " + (System.nanoTime() - t0) / 1000000 + " ms");
                nativeCommitResult(commitReq, 0, name);
            } catch (IOException | RuntimeException e) {
                Log.w(TAG, "saf: commit failed", e);
                nativeCommitResult(commitReq, 2, String.valueOf(e.getMessage()));
            }
        });
    }

    /** The size the provider reports for {@code uri}, or -1 when it does not say. */
    private long docSize(Uri uri) {
        try (android.database.Cursor c = getContentResolver().query(uri,
                new String[] {android.provider.OpenableColumns.SIZE}, null, null, null)) {
            if (c != null && c.moveToFirst() && !c.isNull(0)) {
                return c.getLong(0);
            }
        } catch (RuntimeException e) {
            // The provider does not answer: the byte count of the copy is all there is to check.
        }
        return -1;
    }

    /** Copies {@code in} to {@code out} and returns the number of bytes written. Closes both. */
    private static long copyCounting(InputStream in, OutputStream out) throws IOException {
        long total = 0;
        try (InputStream i = in; OutputStream o = out) {
            final byte[] buf = new byte[1 << 20];
            int n;
            while ((n = i.read(buf)) > 0) {
                o.write(buf, 0, n);
                total += n;
            }
            o.flush();
        }
        return total;
    }

    private OutputStream openForWrite(Uri uri) throws IOException {
        final ContentResolver cr = getContentResolver();
        OutputStream out;
        try {
            out = cr.openOutputStream(uri, "wt");
        } catch (RuntimeException | java.io.FileNotFoundException e) {
            out = cr.openOutputStream(uri, "w"); // some providers have no truncate mode
        }
        if (out == null) {
            throw new IOException("The file could not be opened for writing.");
        }
        return out;
    }

    /**
     * Write {@code file} into the document. A document that already has content is copied to the
     * app cache first. If the write fails or the size afterwards is not the size of {@code file},
     * the old content is put back and the error is passed on, so a failed save does not leave a
     * truncated file behind.
     */
    private void writeTo(Uri uri, File file) throws IOException {
        final ContentResolver cr = getContentResolver();
        final long expected = file.length();
        File backup = null;
        final long before = docSize(uri);
        if (before > 0) {
            backup = File.createTempFile("keep", ".bak", getCacheDir());
            try {
                final InputStream in = cr.openInputStream(uri);
                if (in == null) {
                    throw new IOException("no stream");
                }
                copyCounting(in, new FileOutputStream(backup));
            } catch (IOException | RuntimeException e) {
                // The old content cannot be read, so it cannot be restored: do not risk it.
                backup.delete();
                throw new IOException("The existing file could not be read, so it was not replaced.");
            }
        }
        try {
            final long written = copyCounting(new FileInputStream(file), openForWrite(uri));
            final long size = docSize(uri);
            if (written != expected || (size >= 0 && size != expected)) {
                throw new IOException("The file was not saved completely (" + written + " of "
                        + expected + " bytes).");
            }
        } catch (IOException | RuntimeException e) {
            if (backup != null) {
                try {
                    copyCounting(new FileInputStream(backup), openForWrite(uri));
                } catch (IOException | RuntimeException restore) {
                    Log.w(TAG, "saf: could not restore the old content", restore);
                }
            }
            if (e instanceof IOException) {
                throw (IOException) e;
            }
            throw new IOException(String.valueOf(e.getMessage()));
        } finally {
            if (backup != null) {
                backup.delete();
            }
        }
    }

    private void deleteQuietly(Uri doc) {
        try {
            DocumentsContract.deleteDocument(getContentResolver(), doc);
        } catch (RuntimeException | java.io.FileNotFoundException e) {
            Log.w(TAG, "saf: could not remove the partial file", e);
        }
    }

    /**
     * Create each file in the persisted export folder (inside {@code subfolder} when it is not
     * empty) and copy it. The provider renames on a clash ("Name (1).png").
     */
    public void commitIntoTree(final int commitReq, final String[] files, final String subfolder) {
        copier.execute(() -> {
            final String s = prefs().getString(PREF_TREE, "");
            if (s.isEmpty() || treeState().isEmpty()) {
                nativeCommitResult(commitReq, 2, "The export folder is not available. Choose it again.");
                return;
            }
            try {
                final long t0 = System.nanoTime();
                final ContentResolver cr = getContentResolver();
                final Uri tree = Uri.parse(s);
                Uri parent = DocumentsContract.buildDocumentUriUsingTree(tree,
                        DocumentsContract.getTreeDocumentId(tree));
                String shown = null;
                if (subfolder != null && !subfolder.isEmpty()) {
                    final Uri dir = DocumentsContract.createDocument(cr, parent,
                            DocumentsContract.Document.MIME_TYPE_DIR, subfolder);
                    if (dir == null) {
                        throw new IOException("The folder could not be created.");
                    }
                    parent = dir;
                    shown = displayName(dir);
                }
                for (String path : files) {
                    final File f = new File(path);
                    final Uri doc = DocumentsContract.createDocument(cr, parent, mimeFor(f.getName()), f.getName());
                    if (doc == null) {
                        throw new IOException("The file could not be created.");
                    }
                    try {
                        writeTo(doc, f);
                    } catch (IOException | RuntimeException e) {
                        deleteQuietly(doc); // a new document: no partial file is left in the folder
                        throw e;
                    }
                    if (shown == null) {
                        shown = displayName(doc);
                    }
                }
                Log.i(TAG, "saf: committed " + shown + " in " + (System.nanoTime() - t0) / 1000000 + " ms");
                nativeCommitResult(commitReq, 0, shown == null ? "" : shown);
            } catch (IOException | RuntimeException e) {
                Log.w(TAG, "saf: commit into tree failed", e);
                nativeCommitResult(commitReq, 2, String.valueOf(e.getMessage()));
            }
        });
    }

    /** MIME type for createDocument: unknown types are octet-stream so providers keep the name. */
    private static String mimeFor(String name) {
        final int dot = name.lastIndexOf('.');
        final String ext = dot < 0 ? "" : name.substring(dot + 1).toLowerCase(java.util.Locale.ROOT);
        switch (ext) {
            case "png": return "image/png";
            case "jpg":
            case "jpeg": return "image/jpeg";
            case "webp": return "image/webp";
            case "gif": return "image/gif";
            case "tif":
            case "tiff": return "image/tiff";
            case "pdf": return "application/pdf";
            case "psd": return "image/vnd.adobe.photoshop";
            case "glb": return "model/gltf-binary";
            case "usdz": return "model/vnd.usdz+zip";
            default: return "application/octet-stream";
        }
    }

    // ---- share sheet ----

    /** ACTION_SEND or ACTION_SEND_MULTIPLE with provider URIs, in the system chooser. */
    public void shareFiles(final String[] files, final String mime, final String title) {
        runOnUiThread(() -> {
            try {
                final ArrayList<Uri> uris = new ArrayList<>();
                for (String f : files) {
                    final Uri u = ExportProvider.uriFor(this, new File(f));
                    if (u == null) {
                        nativeNotice("That file cannot be shared.");
                        return;
                    }
                    uris.add(u);
                }
                final Intent send = new Intent(uris.size() == 1 ? Intent.ACTION_SEND : Intent.ACTION_SEND_MULTIPLE);
                send.setType(mime == null || mime.isEmpty() ? "*/*" : mime);
                if (uris.size() == 1) {
                    send.putExtra(Intent.EXTRA_STREAM, uris.get(0));
                } else {
                    send.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
                }
                final ClipData clip = ClipData.newRawUri("Eskiz export", uris.get(0));
                for (int k = 1; k < uris.size(); k++) {
                    clip.addItem(new ClipData.Item(uris.get(k)));
                }
                send.setClipData(clip);
                send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                Log.i(TAG, "share: " + uris.size() + " file(s) " + send.getType());
                startActivity(Intent.createChooser(send, title));
            } catch (RuntimeException e) {
                Log.w(TAG, "share: failed", e);
                nativeNotice("Sharing is not available: " + e.getMessage());
            }
        });
    }

    // ---- clipboard ----

    /** Put the PNG at {@code png} (under cacheDir/share/clip) on the clipboard as copy {@code serial}. */
    public void setClipImage(final String png, final long serial) {
        runOnUiThread(() -> {
            try {
                final Uri u = ExportProvider.uriFor(this, new File(png));
                if (u == null) {
                    return;
                }
                final ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
                cm.setPrimaryClip(ClipData.newUri(getContentResolver(), CLIP_LABEL + serial, u));
                Log.i(TAG, "clip: put " + serial);
            } catch (RuntimeException e) {
                Log.w(TAG, "clip: put failed", e);
            }
        });
    }

    /** {@code none}, {@code text}, {@code foreign} or {@code ours:<serial>}. Quick; no clip data is read. */
    public String clipState() {
        try {
            final ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            if (!cm.hasPrimaryClip()) {
                return "none";
            }
            final ClipDescription d = cm.getPrimaryClipDescription();
            if (d == null) {
                return "none";
            }
            if (d.hasMimeType("image/*")) {
                final CharSequence label = d.getLabel();
                if (label != null && label.toString().startsWith(CLIP_LABEL)) {
                    try {
                        return "ours:" + Long.parseLong(label.toString().substring(CLIP_LABEL.length()));
                    } catch (NumberFormatException e) {
                        return "foreign";
                    }
                }
                return "foreign";
            }
            if (d.hasMimeType("text/*")) {
                return "text";
            }
        } catch (RuntimeException e) {
            // no access to the clipboard now
        }
        return "none";
    }

    /** Copy the image another app put on the clipboard to {@code dir/clip-in}. */
    public void readClipImage(final int req, final String dir) {
        runOnUiThread(() -> {
            final Uri uri;
            try {
                final ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
                final ClipData clip = cm.getPrimaryClip();
                if (clip == null || clip.getItemCount() == 0 || clip.getItemAt(0).getUri() == null) {
                    nativeClipResult(req, 2, "", "There is no image on the clipboard.");
                    return;
                }
                uri = clip.getItemAt(0).getUri();
            } catch (RuntimeException e) {
                nativeClipResult(req, 2, "", "The clipboard cannot be read now.");
                return;
            }
            copier.execute(() -> {
                try {
                    final File folder = new File(dir);
                    if (!folder.isDirectory() && !folder.mkdirs()) {
                        throw new IOException("The cache folder is not available.");
                    }
                    final File out = new File(folder, "clip-in");
                    try (InputStream in = getContentResolver().openInputStream(uri)) {
                        if (in == null) {
                            throw new IOException("The image could not be opened.");
                        }
                        copyStream(in, new FileOutputStream(out));
                    }
                    Log.i(TAG, "clip: read " + out.length() + " bytes");
                    nativeClipResult(req, 0, out.getAbsolutePath(), "");
                } catch (IOException | RuntimeException e) {
                    Log.w(TAG, "clip: read failed", e);
                    nativeClipResult(req, 2, "", "The image could not be read.");
                }
            });
        });
    }

    // ---- Back ----

    /** Rust has something Back can close (true), or Back should leave the app (false). */
    public void setBackWanted(final boolean wanted) {
        runOnUiThread(() -> {
            backWanted = wanted;
            if (android.os.Build.VERSION.SDK_INT >= 33) {
                final OnBackInvokedDispatcher disp = getOnBackInvokedDispatcher();
                if (wanted && !backRegistered) {
                    if (backCallback == null) {
                        backCallback = () -> nativeBack();
                    }
                    disp.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, backCallback);
                    backRegistered = true;
                } else if (!wanted && backRegistered) {
                    disp.unregisterOnBackInvokedCallback(backCallback);
                    backRegistered = false;
                }
            }
        });
    }

    /** API 29 to 32 (and 33+ without predictive back): the old Back path. */
    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        if (backWanted) {
            nativeBack();
        } else {
            super.onBackPressed();
        }
    }

    // ---- test hooks and housekeeping ----

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleTestIntent(intent);
    }

    /** {@code --es eskiz_test <hook>}: debuggable APKs only. */
    private void handleTestIntent(Intent intent) {
        if (intent == null) {
            return;
        }
        final String hook = intent.getStringExtra("eskiz_test");
        if (hook == null) {
            return;
        }
        intent.removeExtra("eskiz_test");
        if (debuggable()) {
            Log.i(TAG, "eskiz-test: hook " + hook);
            nativeTestHook(hook);
        } else {
            Log.i(TAG, "eskiz-test: ignored (release)");
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        // Open pickers survive a process death: keep what the copy needs, so the result can be
        // answered (Rust no longer knows the id and says so).
        final ArrayList<String> rows = new ArrayList<>();
        for (java.util.Map.Entry<Integer, Integer> e : kinds.entrySet()) {
            rows.add(e.getKey() + "|" + e.getValue() + "|" + dirs.get(e.getKey()));
        }
        out.putStringArrayList("eskiz_requests", rows);
    }

    private void restoreRequests(Bundle saved) {
        if (saved == null) {
            return;
        }
        final List<String> rows = saved.getStringArrayList("eskiz_requests");
        if (rows == null) {
            return;
        }
        for (String r : rows) {
            final String[] p = r.split("\\|", 3);
            if (p.length == 3) {
                try {
                    kinds.put(Integer.parseInt(p[0]), Integer.parseInt(p[1]));
                    dirs.put(Integer.parseInt(p[0]), p[2]);
                } catch (NumberFormatException e) {
                    // ignore a damaged row
                }
            }
        }
    }

    /** Delete cache entries older than 24 hours, except the clipboard file in use. */
    private void cleanCacheLater() {
        copier.execute(() -> {
            final String state = clipState();
            final String keep = state.startsWith("ours:") ? "clip-" + state.substring(5) + ".png" : "";
            final long cutoff = System.currentTimeMillis() - 24L * 3600 * 1000;
            deleteOld(new File(getCacheDir(), "saf"), cutoff, keep);
            deleteOld(new File(getCacheDir(), "share"), cutoff, keep);
        });
    }

    private static void deleteOld(File dir, long cutoff, String keep) {
        final File[] list = dir.listFiles();
        if (list == null) {
            return;
        }
        for (File f : list) {
            if (f.getName().equals(keep)) {
                continue;
            }
            if (f.isDirectory()) {
                deleteOld(f, cutoff, keep);
                final String[] left = f.list();
                if (left != null && left.length == 0 && f.lastModified() < cutoff) {
                    f.delete();
                }
            } else if (f.lastModified() < cutoff) {
                f.delete();
            }
        }
    }
}
