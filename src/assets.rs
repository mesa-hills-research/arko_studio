//! The app's assets: GPUI Kit's default icons, plus the Lucide icons Arko
//! Studio uses beyond them, embedded from the kit's set by name.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

// The composer's and the dialogs' (Plug and MessageCircleQuestionMark come
// with the transcript's).
gpui_kit::assets::icon_assets!(pub ExtraIcons, [Paperclip, Maximize2, Minimize2, MessageSquare, KeyRound]);
// The transcript's: tool icons, the question label, and Edit, Branch and
// Regenerate.
gpui_kit::assets::icon_assets!(pub TranscriptIcons, [FilePen, FolderTree, Plug, Wrench, Activity, MessageCircleQuestionMark, SquarePen, GitBranch, RotateCcw]);
// The status bar's cache timer.
gpui_kit::assets::icon_assets!(pub StatusIcons, [Timer]);
// The worker profiles page and its editor, and the sidebar's mark of a
// session that needs approval.
gpui_kit::assets::icon_assets!(pub ProfileIcons, [UserCog, Trash, ShieldCheck]);

/// The icons the app draws, for `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = TranscriptIcons.load(path)? {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = StatusIcons.load(path)? {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = ProfileIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(TranscriptIcons.list(path)?);
        paths.extend(StatusIcons.list(path)?);
        paths.extend(ProfileIcons.list(path)?);
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
        for icon in [
            IconName::Paperclip,
            IconName::Maximize2,
            IconName::Plus,
            IconName::Timer,
            // A message's Edit and Branch, and Regenerate.
            IconName::SquarePen,
            IconName::GitBranch,
            IconName::RotateCcw,
            // A monitor's calls.
            IconName::Activity,
            // The worker profiles page.
            IconName::UserCog,
            IconName::Trash,
            // A session that needs approval.
            IconName::ShieldCheck,
        ] {
            assert!(
                Assets.load(&icon.path()).unwrap().is_some(),
                "{}",
                icon.path()
            );
        }
    }
}
