package app.eskiz.sharetest;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;

/** Serves {@code cacheDir/clip.png} (read-only) for the clipboard test image. */
public class TestProvider extends ContentProvider {
    static final String AUTHORITY = "app.eskiz.sharetest.files";

    private File file(Uri uri) throws FileNotFoundException {
        try {
            final File root = getContext().getCacheDir().getCanonicalFile();
            final File f = new File(root, uri.getLastPathSegment()).getCanonicalFile();
            if (!f.getPath().startsWith(root.getPath() + File.separator) || !f.isFile()) {
                throw new FileNotFoundException(String.valueOf(uri));
            }
            return f;
        } catch (IOException e) {
            throw new FileNotFoundException(String.valueOf(uri));
        }
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        return ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Cursor query(Uri uri, String[] p, String s, String[] a, String o) {
        try {
            final File f = file(uri);
            final MatrixCursor c = new MatrixCursor(new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE});
            c.addRow(new Object[] {f.getName(), f.length()});
            return c;
        } catch (FileNotFoundException e) {
            return null;
        }
    }

    @Override
    public String getType(Uri uri) {
        return "image/png";
    }

    @Override
    public Uri insert(Uri uri, ContentValues v) {
        throw new UnsupportedOperationException();
    }

    @Override
    public int delete(Uri uri, String s, String[] a) {
        throw new UnsupportedOperationException();
    }

    @Override
    public int update(Uri uri, ContentValues v, String s, String[] a) {
        throw new UnsupportedOperationException();
    }
}
