//! The app's assets: GPUI Kit's default icons, plus the Lucide icons Arko
//! Studio uses beyond them, embedded from the kit's set by name.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(pub ExtraIcons, [Paperclip, Maximize2, Minimize2, MessageSquare, MessageCircleQuestionMark, KeyRound, Plug]);

/// The icons the app draws, for `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    #[test]
    fn the_extra_icons_and_the_defaults_load() {
        for icon in [IconName::Paperclip, IconName::Maximize2, IconName::Plus] {
            assert!(
                Assets.load(&icon.path()).unwrap().is_some(),
                "{}",
                icon.path()
            );
        }
    }
}
