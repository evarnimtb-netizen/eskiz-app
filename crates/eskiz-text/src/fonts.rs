//! Font library: installed fonts, imported files and folders, and the missing-font
//! fallback order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cosmic_text::{fontdb, Font, FontSystem};
use thiserror::Error;

use crate::model::FontFace;

/// Fallback line metrics as ratios of the em size: ascent, descent and leading.
pub(crate) const FALLBACK_RATIOS: (f32, f32, f32) = (0.8, 0.2, 0.0);

/// Errors from importing fonts or resolving families.
#[derive(Debug, Error)]
pub enum FontError {
    #[error("could not read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("\"{file}\" is not a usable font: {reason}")]
    Invalid { file: String, reason: String },
    #[error("\"{file}\" is not a font file Eskiz can import (expected .ttf or .otf)")]
    Unsupported { file: String },
    #[error("no installed or imported font family named \"{0}\"")]
    UnknownFamily(String),
}

/// The outcome of resolving a run's family.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    /// The family to lay out with.
    pub family: String,
    /// Multiplier for the run's size: the replacement's relative size, or 1.0.
    pub size_scale: f32,
    /// True when the requested family is not installed. The layer is then marked "missing fonts".
    pub missing: bool,
}

/// A user-chosen substitute for a missing family, with a relative size.
#[derive(Clone, Debug, PartialEq)]
pub struct Replacement {
    pub family: String,
    pub relative_size: f32,
}

/// Result of loading a folder of fonts.
#[derive(Debug, Default)]
pub struct DirReport {
    /// Family names of the faces that were loaded, sorted and unique.
    pub families: Vec<String>,
    /// Files that were found but could not be loaded, with the reason for each.
    pub rejected: Vec<FontError>,
}

/// Where Android keeps its system fonts. fontdb scans no font folder on Android, so the library
/// loads these itself (every folder that exists is loaded).
pub const ANDROID_FONT_DIRS: [&str; 3] = ["/system/fonts", "/product/fonts", "/system/product/fonts"];

/// Installed fonts, imported fonts and the fallback table.
pub struct FontLibrary {
    system: FontSystem,
    default_family: Option<String>,
    replacements: BTreeMap<String, Replacement>,
}

impl Default for FontLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl FontLibrary {
    /// Loads the system fonts. The default family is the system sans-serif.
    pub fn new() -> Self {
        let mut library = Self {
            system: FontSystem::new(),
            default_family: None,
            replacements: BTreeMap::new(),
        };
        if cfg!(target_os = "android") {
            library.load_system_dirs(ANDROID_FONT_DIRS.iter().map(Path::new));
        }
        library.default_family = library.choose_default_family();
        library
    }

    /// A library holding only the fonts found in `dirs` (folders that do not exist are skipped).
    /// This is what [`FontLibrary::new`] does on Android with [`ANDROID_FONT_DIRS`], with the
    /// folders injected so a host test can stand in for `/system/fonts`.
    pub fn with_font_dirs<'a>(dirs: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut library = Self {
            system: FontSystem::new_with_locale_and_db("en-US".to_owned(), fontdb::Database::new()),
            default_family: None,
            replacements: BTreeMap::new(),
        };
        library.load_system_dirs(dirs);
        library.default_family = library.choose_default_family();
        library
    }

    fn load_system_dirs<'a>(&mut self, dirs: impl IntoIterator<Item = &'a Path>) {
        for dir in dirs.into_iter().filter(|d| d.is_dir()) {
            self.system.db_mut().load_fonts_dir(dir);
        }
    }

    /// The generic sans-serif family if fontdb knows one installed. Otherwise the first preferred
    /// family that is installed, then any family at all. fontdb's own default (Arial) is missing
    /// on macOS, because fontdb does not scan `/System/Library/Fonts/Supplemental`.
    fn choose_default_family(&self) -> Option<String> {
        const PREFERRED: [&str; 7] = [
            "Helvetica",
            "Arial",
            "Helvetica Neue",
            "Liberation Sans",
            "DejaVu Sans",
            "Noto Sans",
            "Roboto",
        ];
        self.generic_family(fontdb::Family::SansSerif)
            .filter(|name| self.has_family(name))
            .or_else(|| {
                PREFERRED
                    .iter()
                    .find(|name| self.has_family(name))
                    .map(|name| name.to_string())
            })
            .or_else(|| self.families().into_iter().next())
    }

    fn generic_family(&self, family: fontdb::Family<'_>) -> Option<String> {
        let query = fontdb::Query {
            families: &[family],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        let db = self.system.db();
        let id = db.query(&query)?;
        db.face(id)?.families.first().map(|(name, _)| name.clone())
    }

    /// The family used for missing fonts and for the empty (default) family name.
    pub fn default_family(&self) -> Option<&str> {
        self.default_family.as_deref()
    }

    /// Every family name loaded, sorted and unique.
    pub fn families(&self) -> Vec<String> {
        let names: BTreeSet<String> = self
            .system
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.into_iter().collect()
    }

    fn query_id(&self, family: &str, face: FontFace) -> Option<fontdb::ID> {
        let families = [fontdb::Family::Name(family)];
        let query = fontdb::Query {
            families: &families,
            weight: weight_of(face),
            stretch: fontdb::Stretch::Normal,
            style: if face.is_italic() {
                fontdb::Style::Italic
            } else {
                fontdb::Style::Normal
            },
        };
        self.system.db().query(&query)
    }

    /// True when a family of this name is installed or imported.
    pub fn has_family(&self, family: &str) -> bool {
        !family.is_empty() && self.query_id(family, FontFace::Regular).is_some()
    }

    /// Resolves a run's family with the documented fallback order: the installed family, then the
    /// user's replacement at its relative size, then the default family at the same size.
    pub fn resolve(&self, family: &str) -> Resolution {
        let default = self.default_family.clone().unwrap_or_default();
        if family.is_empty() {
            return Resolution {
                family: default,
                size_scale: 1.0,
                missing: false,
            };
        }
        if self.has_family(family) {
            return Resolution {
                family: family.to_string(),
                size_scale: 1.0,
                missing: false,
            };
        }
        match self.replacements.get(family) {
            Some(rep) if self.has_family(&rep.family) => Resolution {
                family: rep.family.clone(),
                size_scale: rep.relative_size,
                missing: true,
            },
            _ => Resolution {
                family: default,
                size_scale: 1.0,
                missing: true,
            },
        }
    }

    /// Maps a missing family to an installed one, at `relative_size` times the run's size.
    pub fn set_replacement(
        &mut self,
        missing: &str,
        with: &str,
        relative_size: f32,
    ) -> Result<(), FontError> {
        if !self.has_family(with) {
            return Err(FontError::UnknownFamily(with.to_string()));
        }
        let relative_size = if relative_size.is_finite() && relative_size > 0.0 {
            relative_size
        } else {
            1.0
        };
        self.replacements.insert(
            missing.to_string(),
            Replacement {
                family: with.to_string(),
                relative_size,
            },
        );
        Ok(())
    }

    /// Removes the replacement for a family.
    pub fn clear_replacement(&mut self, missing: &str) {
        self.replacements.remove(missing);
    }

    /// The replacement mapped to a family, if any.
    pub fn replacement(&self, missing: &str) -> Option<&Replacement> {
        self.replacements.get(missing)
    }

    /// Imports one `.ttf`, `.otf` or `.ttc` file. Returns the families it added. A file that is
    /// not a valid font is rejected, and the error names the file.
    pub fn import_file(&mut self, path: &Path) -> Result<Vec<String>, FontError> {
        let file = file_name(path);
        let is_supported = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ["ttf", "otf", "ttc"].iter().any(|e| ext.eq_ignore_ascii_case(e)));
        if !is_supported {
            return Err(FontError::Unsupported { file });
        }
        let data = std::fs::read(path).map_err(|source| FontError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        validate(&data, &file)?;
        Ok(self.add_font_data(data))
    }

    /// Loads every `.ttf`, `.otf` and `.ttc` file in a folder. Other files are ignored. A file
    /// that fails validation is listed in [`DirReport::rejected`] and does not stop the others.
    ///
    pub fn load_dir(&mut self, dir: &Path) -> Result<DirReport, FontError> {
        let entries = std::fs::read_dir(dir).map_err(|source| FontError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file() && is_font_file(path))
            .collect();
        paths.sort();

        let mut report = DirReport::default();
        for path in paths {
            let file = file_name(&path);
            match std::fs::read(&path) {
                Err(source) => report.rejected.push(FontError::Io { path, source }),
                Ok(data) => match validate(&data, &file) {
                    Err(error) => report.rejected.push(error),
                    Ok(()) => report.families.extend(self.add_font_data(data)),
                },
            }
        }
        report.families.sort();
        report.families.dedup();
        Ok(report)
    }

    fn add_font_data(&mut self, data: Vec<u8>) -> Vec<String> {
        let before: Vec<fontdb::ID> = self.system.db().faces().map(|face| face.id).collect();
        self.system.db_mut().load_font_data(data);
        let added: BTreeSet<String> = self
            .system
            .db()
            .faces()
            .filter(|face| !before.contains(&face.id))
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        added.into_iter().collect()
    }

    pub(crate) fn system_mut(&mut self) -> &mut FontSystem {
        &mut self.system
    }

    pub(crate) fn font(&mut self, id: fontdb::ID, weight: fontdb::Weight) -> Option<Arc<Font>> {
        self.system.get_font(id, weight)
    }

    pub(crate) fn ratios_by_id(
        &mut self,
        id: fontdb::ID,
        weight: fontdb::Weight,
    ) -> (f32, f32, f32) {
        match self.system.get_font(id, weight) {
            Some(font) => ratios_of(&font),
            None => FALLBACK_RATIOS,
        }
    }

    pub(crate) fn ratios_for_face(&mut self, family: &str, face: FontFace) -> (f32, f32, f32) {
        match self.query_id(family, face) {
            Some(id) => self.ratios_by_id(id, weight_of(face)),
            None => FALLBACK_RATIOS,
        }
    }
}

fn weight_of(face: FontFace) -> fontdb::Weight {
    if face.is_bold() {
        fontdb::Weight::BOLD
    } else {
        fontdb::Weight::NORMAL
    }
}

/// Ascent, descent and leading as ratios of the em size.
fn ratios_of(font: &Font) -> (f32, f32, f32) {
    let metrics = font.as_swash().metrics(&[]);
    let em = f32::from(metrics.units_per_em.max(1));
    (
        metrics.ascent / em,
        metrics.descent.abs() / em,
        metrics.leading / em,
    )
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// Checks the file signature and that the font tables parse.
fn validate(data: &[u8], file: &str) -> Result<(), FontError> {
    let head = data.get(0..4).unwrap_or(&[]);
    let known_signature = matches!(head, [0, 1, 0, 0] | b"true" | b"OTTO" | b"ttcf");
    if !known_signature {
        return Err(FontError::Invalid {
            file: file.to_string(),
            reason: "not a TrueType or OpenType file".to_string(),
        });
    }
    if swash::FontRef::from_index(data, 0).is_none() {
        return Err(FontError::Invalid {
            file: file.to_string(),
            reason: "the font tables could not be read".to_string(),
        });
    }
    Ok(())
}
