//! Fonts: the ones Arko Studio bundles, and the font files the user adds,
//! which it keeps in `<arko folder>/fonts` and registers at start-up.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow};
use gpui_kit::component::font_picker::FontCatalog;
use gpui_kit::{App, BorrowAppContext as _, Global, SharedString};

/// Font files Arko Studio reads from the fonts folder.
const FONT_EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

/// The fonts Arko Studio bundles, from GPUI Kit: Work Sans for the
/// interface and JetBrains Mono for code, both variable, upright and italic.
pub fn bundled() -> Vec<Cow<'static, [u8]>> {
    gpui_kit::assets::fonts::all()
}

/// The font files the user added earlier, read from `folder`.
pub fn added(folder: &Path) -> Vec<Cow<'static, [u8]>> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| is_font_file(path))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| match std::fs::read(&path) {
            Ok(data) => Some(Cow::Owned(data)),
            Err(error) => {
                tracing::warn!("couldn't read the font {}: {error}", path.display());
                None
            }
        })
        .collect()
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| FONT_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

/// The fonts the app registered, kept for the font pickers, and the
/// families the text system can draw.
#[derive(Default)]
pub struct Fonts {
    bundled: Vec<Cow<'static, [u8]>>,
    added: Vec<Cow<'static, [u8]>>,
    /// Lowercased family names, read once and again after a font is added.
    families: Option<HashSet<String>>,
}

impl Global for Fonts {}

impl Fonts {
    pub fn bundled(&self) -> Vec<Cow<'static, [u8]>> {
        self.bundled.clone()
    }

    pub fn added(&self) -> Vec<Cow<'static, [u8]>> {
        self.added.clone()
    }
}

/// Registers the bundled fonts and the ones the user added.
pub fn init(bundled: Vec<Cow<'static, [u8]>>, added: Vec<Cow<'static, [u8]>>, cx: &mut App) {
    for font in bundled.iter().chain(added.iter()) {
        if let Err(error) = cx.text_system().add_fonts(vec![font.clone()]) {
            tracing::error!("couldn't load a font: {error}");
        }
    }
    cx.set_global(Fonts {
        bundled,
        added,
        families: None,
    });
}

/// Whether the text system can draw `family`.
pub fn is_available(family: &str, cx: &mut App) -> bool {
    if !cx.has_global::<Fonts>() {
        cx.set_global(Fonts::default());
    }
    if cx.global::<Fonts>().families.is_none() {
        let names = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .map(|name| name.to_lowercase())
            .collect();
        cx.global_mut::<Fonts>().families = Some(names);
    }
    cx.global::<Fonts>()
        .families
        .as_ref()
        .is_some_and(|names| names.contains(&family.to_lowercase()))
}

/// Records a font file the user just added, which a font picker registered
/// with the text system: the font pickers made later list it too.
pub fn note_added(data: Cow<'static, [u8]>, cx: &mut App) {
    if !cx.has_global::<Fonts>() {
        cx.set_global(Fonts::default());
    }
    let fonts = cx.global_mut::<Fonts>();
    fonts.added.push(data.clone());
    fonts.families = None;
    if cx.has_global::<PickerCatalog>() {
        cx.update_global::<PickerCatalog, _>(|picker, _| {
            if let Some(catalog) = picker.catalog.as_mut() {
                catalog.add_fonts(&[data]);
            }
        });
    }
}

/// The families the font pickers list: the system's, the bundled ones and
/// the ones the user added, with their faces. Reading them takes a while
/// (every installed font file is opened), so it's done once, in the
/// background, the first time Settings opens, and the pickers wait for it.
#[derive(Default)]
pub struct PickerCatalog {
    catalog: Option<FontCatalog>,
    loading: Option<gpui_kit::Task<()>>,
}

impl Global for PickerCatalog {}

/// The fonts the pickers list, once they're read ([`load_picker_catalog`]).
pub fn picker_catalog(cx: &App) -> Option<FontCatalog> {
    cx.try_global::<PickerCatalog>()?.catalog.clone()
}

/// Starts reading the fonts the pickers list, unless that's done or under
/// way. Observers of [`PickerCatalog`] hear when it's ready.
pub fn load_picker_catalog(cx: &mut App) {
    let picker = cx.default_global::<PickerCatalog>();
    if picker.catalog.is_some() || picker.loading.is_some() {
        return;
    }
    let (bundled, added) = cx
        .try_global::<Fonts>()
        .map(|fonts| (fonts.bundled(), fonts.added()))
        .unwrap_or_default();
    let system = FontCatalog::load(cx);
    let loading = cx.spawn(async move |cx| {
        let system = system.await;
        // The faces of the fonts the app registered from its own data are in
        // that data, not in the installed files.
        let catalog = cx
            .background_executor()
            .spawn(async move {
                let mut catalog = system;
                catalog.include_fonts(&bundled);
                catalog.add_fonts(&added);
                catalog
            })
            .await;
        cx.update(|cx| {
            cx.update_global::<PickerCatalog, _>(|picker, _| {
                picker.catalog = Some(catalog);
                picker.loading = None;
            })
        });
    });
    cx.global_mut::<PickerCatalog>().loading = Some(loading);
}

/// A font file the user chose, copied into the fonts folder.
pub struct ImportedFont {
    pub data: Vec<u8>,
    /// The families in the file.
    pub families: Vec<SharedString>,
    /// The folder had this file already, so it is registered already.
    pub already_imported: bool,
}

/// Copies the font file at `path` into `folder`, keeping its name, unless
/// the folder has the same file already. Blocks: call it off the UI thread.
pub fn import(path: &Path, folder: &Path) -> anyhow::Result<ImportedFont> {
    let data = std::fs::read(path).with_context(|| format!("Couldn’t read {}.", path.display()))?;
    let families: Vec<SharedString> = FontCatalog::from_fonts(&[&data])
        .families()
        .iter()
        .map(|family| family.name().clone())
        .collect();
    if families.is_empty() {
        return Err(anyhow!("{} isn’t a font file.", file_name(path)));
    }

    std::fs::create_dir_all(folder)
        .with_context(|| format!("Couldn’t make {}.", folder.display()))?;
    let mut target = folder.join(file_name(path));
    let mut copy = 1;
    while target.exists() {
        if std::fs::read(&target).is_ok_and(|existing| existing == data) {
            return Ok(ImportedFont {
                data,
                families,
                already_imported: true,
            });
        }
        copy += 1;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("font");
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("ttf");
        target = folder.join(format!("{stem} ({copy}).{extension}"));
    }
    std::fs::write(&target, &data)
        .with_context(|| format!("Couldn’t copy the font to {}.", target.display()))?;
    Ok(ImportedFont {
        data,
        families,
        already_imported: false,
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "font".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_isnt_a_font_is_refused_and_not_copied() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("notes.ttf");
        std::fs::write(&source, b"not a font").unwrap();
        let fonts = folder.path().join("fonts");
        let error = import(&source, &fonts).err().unwrap();
        assert_eq!(error.to_string(), "notes.ttf isn’t a font file.");
        assert!(!fonts.exists());
    }

    #[test]
    fn only_font_files_are_read_from_the_folder() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("a.TTF"), b"1").unwrap();
        std::fs::write(folder.path().join("b.otf"), b"2").unwrap();
        std::fs::write(folder.path().join("readme.txt"), b"3").unwrap();
        let read: Vec<Vec<u8>> = added(folder.path())
            .into_iter()
            .map(|data| data.into_owned())
            .collect();
        assert_eq!(read, [b"1".to_vec(), b"2".to_vec()]);
        assert!(added(&folder.path().join("missing")).is_empty());
    }
}
