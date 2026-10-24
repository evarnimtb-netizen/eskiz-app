package app.eskiz;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;

/**
 * Eskiz's own small read-only content provider (A4). It serves files under {@code cacheDir/share/}
 * to the system share sheet and to the clipboard, and nothing else. It replaces androidx.core's
 * FileProvider, because the app has no AndroidX dependency.
 *
 * <ul>
 *   <li>Authority {@code app.eskiz.exports}, not exported, {@code grantUriPermissions="true"}: a
 *       receiving app can read only the URIs it was handed (intent flag or clipboard grant).</li>
 *   <li>{@link #openFile} is read-only. The path is resolved to a canonical file and refused unless
 *       it is strictly inside the root (no {@code ..}, no symlink that leads out).</li>
 *   <li>{@link #query} answers {@code _display_name} and {@code _size}; {@link #getType} maps the
 *       extension.</li>
 * </ul>
 */
public class ExportProvider extends ContentProvider {
    public static final String AUTHORITY = "app.eskiz.exports";

    /** The folder this provider serves: {@code cacheDir/share}. */
    static File root(Context c) {
        return new File(c.getCacheDir(), "share");
    }

    /** The content URI of a file under the root, or null when it is not inside it. */
    static Uri uriFor(Context c, File file) {
        try {
            final String rootPath = root(c).getCanonicalPath();
            final String path = file.getCanonicalPath();
            if (!path.startsWith(rootPath + File.separator)) {
                return null;
            }
            final String rel = path.substring(rootPath.length() + 1);
            final Uri.Builder b = new Uri.Builder().scheme("content").authority(AUTHORITY);
            for (String part : rel.split(File.separator.equals("\\") ? "\\\\" : File.separator)) {
                b.appendPath(part);
            }
            return b.build();
        } catch (IOException e) {
            return null;
        }
    }

    /**
     * The rule: is {@code candidate} strictly inside {@code root}? Both must be canonical. The same
     * rule as {@code saf::inside_root} in Rust, which is covered by a host test.
     */
    static boolean insideRoot(String root, String candidate) {
        return candidate.startsWith(root + File.separator);
    }

    /** The file a URI names, or null when it is outside the root or malformed. */
    private File resolve(Uri uri) {
        if (uri == null || !AUTHORITY.equals(uri.getAuthority())) {
            return null;
        }
        final String rel = uri.getPath();
        if (rel == null || rel.length() < 2) {
            return null;
        }
        try {
            final File root = root(getContext());
            final String rootPath = root.getCanonicalPath();
            final File file = new File(root, rel.substring(1)).getCanonicalFile();
            return insideRoot(rootPath, file.getPath()) ? file : null;
        } catch (IOException e) {
            return null;
        }
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) {
            throw new SecurityException("read only");
        }
        final File file = resolve(uri);
        if (file == null || !file.isFile()) {
            throw new FileNotFoundException(String.valueOf(uri));
        }
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] selectionArgs,
            String sortOrder) {
        final File file = resolve(uri);
        if (file == null || !file.isFile()) {
            return null;
        }
        final String[] cols = projection != null ? projection
                : new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE};
        final MatrixCursor c = new MatrixCursor(cols, 1);
        final Object[] row = new Object[cols.length];
        for (int i = 0; i < cols.length; i++) {
            if (OpenableColumns.DISPLAY_NAME.equals(cols[i])) {
                row[i] = file.getName();
            } else if (OpenableColumns.SIZE.equals(cols[i])) {
                row[i] = file.length();
            }
        }
        c.addRow(row);
        return c;
    }

    @Override
    public String getType(Uri uri) {
        final String name = uri.getLastPathSegment();
        if (name == null) {
            return "application/octet-stream";
        }
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
            case "mp4": return "video/mp4";
            default: return "application/octet-stream";
        }
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException("read only");
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("read only");
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("read only");
    }
}
