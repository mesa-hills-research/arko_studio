//! Fonts: the ones Arko Studio bundles, and the font files the user adds,
//! which it keeps in `<arko folder>/fonts` and registers at start-up.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use gpui_kit::component::font_picker::FontCatalog;
use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{
    App, AppContext as _, BorrowAppContext as _, Font, FontFeatures, FontWeight, Global,
    SharedString, Styled, font,
};

/// Font files Arko Studio reads from the fonts folder.
const FONT_EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

/// The code font's OpenType features with ligatures turned off. JetBrains
/// Mono draws its ligatures through `calt`, and other fonts through `liga`.
pub fn no_ligatures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("calt".into(), 0), ("liga".into(), 0)]))
}

/// Sets an element's text in the code font, with the features the "Code
/// ligatures" setting gives it.
pub trait CodeFont: Styled + Sized {
    fn code_font(self, theme: &Theme) -> Self {
        self.font_family(theme.mono_font_family.clone())
            .font_features(theme.mono_font_features.clone())
    }
}

impl<T: Styled> CodeFont for T {}

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
            Ok(data) => Some(keep(data)),
            Err(error) => {
                tracing::warn!("couldn't read the font {} ({error})", path.display());
                None
            }
        })
        .collect()
}

/// A font file's data, kept for as long as the app runs, as the text
/// system keeps it once it's registered: handing it to the font lists then
/// copies no bytes.
pub fn keep(data: Vec<u8>) -> Cow<'static, [u8]> {
    Cow::Borrowed(Box::leak(data.into_boxed_slice()))
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
            tracing::error!("couldn't load a font ({error})");
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

/// Records a font file the user just added and registered with the text
/// system. The font lists made later list it, and so do the ones there,
/// once its families are read in the background. One added while the
/// lists are read waits for them.
pub fn note_added(data: Cow<'static, [u8]>, cx: &mut App) {
    let fonts = cx.default_global::<Fonts>();
    fonts.added.push(data.clone());
    fonts.families = None;
    let picker = cx.default_global::<PickerCatalog>();
    // Before the lists are read, the reading takes it with the other
    // added fonts.
    if picker.catalog.is_none() && picker.loading.is_none() {
        return;
    }
    picker.waiting.push(data);
    add_waiting(cx);
}

/// The families the font pickers list: the system's, the bundled ones and
/// the ones the user added, with their faces. Reading them takes a while
/// (every installed font file is opened), so it's done once, in the
/// background, a moment after the start ([`warm_up`]) or as Settings opens
/// if that's sooner. The pickers wait for it.
#[derive(Default)]
pub struct PickerCatalog {
    catalog: Option<FontCatalog>,
    /// Counts the catalogs: font lists made from an older one are behind.
    version: u64,
    loading: Option<gpui_kit::Task<()>>,
    /// Fonts added while the catalog is read, or while it takes others.
    waiting: Vec<Cow<'static, [u8]>>,
    /// Takes fonts added since the catalog was read.
    adding: Option<gpui_kit::Task<()>>,
}

impl Global for PickerCatalog {}

/// The fonts the pickers list, once they're read ([`load_picker_catalog`]).
pub fn picker_catalog(cx: &App) -> Option<FontCatalog> {
    cx.try_global::<PickerCatalog>()?.catalog.clone()
}

/// Which catalog [`picker_catalog`] gives: a new one has a higher number.
pub fn picker_catalog_version(cx: &App) -> u64 {
    cx.try_global::<PickerCatalog>()
        .map_or(0, |picker| picker.version)
}

/// How long after the start [`warm_up`] gets Settings' fonts ready, once
/// the first window has drawn and the sessions are listed.
pub const WARM_UP_AFTER: Duration = Duration::from_secs(2);

/// A moment after the app starts, gets the fonts Settings shows ready in
/// the background: the catalog its font pickers list, and the fonts its
/// pages draw in. Settings then opens with its pickers at once the first
/// time too, and draws its first frame sooner.
pub fn warm_up(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(WARM_UP_AFTER).await;
        cx.update(|cx| {
            load_picker_catalog(cx);
            let fonts = settings_fonts(cx.theme());
            let text_system = cx.text_system().clone();
            cx.background_spawn(async move { text_system.prewarm_fonts(&fonts) })
                .detach();
        })
    })
    .detach();
}

/// The fonts Settings draws in that the main window may not have used yet:
/// the interface font's regular and medium, and the code font.
fn settings_fonts(theme: &Theme) -> Vec<Font> {
    let interface = |weight| Font {
        weight,
        ..font(theme.font_family.clone())
    };
    let code = Font {
        features: theme.mono_font_features.clone(),
        ..font(theme.mono_font_family.clone())
    };
    vec![
        interface(FontWeight::NORMAL),
        interface(FontWeight::MEDIUM),
        code,
    ]
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
                picker.version += 1;
                picker.loading = None;
            });
            add_waiting(cx);
        });
    });
    cx.global_mut::<PickerCatalog>().loading = Some(loading);
}

/// Adds the fonts waiting to the catalog, in the background, once it's
/// read and isn't taking others.
fn add_waiting(cx: &mut App) {
    let picker = cx.global_mut::<PickerCatalog>();
    if picker.adding.is_some() || picker.waiting.is_empty() {
        return;
    }
    let Some(mut catalog) = picker.catalog.clone() else {
        return;
    };
    let fonts = std::mem::take(&mut picker.waiting);
    let adding = cx.spawn(async move |cx| {
        let catalog = cx
            .background_executor()
            .spawn(async move {
                catalog.add_fonts(&fonts);
                catalog
            })
            .await;
        cx.update(|cx| {
            cx.update_global::<PickerCatalog, _>(|picker, _| {
                picker.catalog = Some(catalog);
                picker.version += 1;
                picker.adding = None;
            });
            add_waiting(cx);
        });
    });
    cx.global_mut::<PickerCatalog>().adding = Some(adding);
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
