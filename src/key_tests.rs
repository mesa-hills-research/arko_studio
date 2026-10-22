//! Shortcuts with Shift on a key that carries a symbol, such as Ctrl+Shift+\,
//! against the keystrokes the platforms send for them.
//!
//! Windows, Linux (X11 and Wayland) and macOS all report Shift and such a key
//! as the shifted symbol alone, without the Shift modifier. Ctrl+Shift+\ on a
//! US keyboard arrives as `ctrl-|`. GPUI's Windows keyboard code does it in
//! `get_keystroke_key`, Linux's in `keystroke_from_xkb` and macOS's in
//! `parse_keystroke`. These tests send that keystroke to a window.

use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::input::{Keymap, KeymapPlatform};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, AsKeystroke as _, Entity, Keystroke, Modifiers,
    TestAppContext,
};

use crate::Startup;
use crate::backend::FakeBackend;
use crate::commands::{commands, delivered_form};
use crate::menus::ToggleWorkersPanel;
use crate::tests::{act, open};
use crate::workspace::Workspace;

fn start(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
}

/// The modifier of the app's own shortcuts on this platform.
fn secondary() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::command()
    } else {
        Modifiers::control()
    }
}

/// What the platform's window sends for Mod+Shift+\ on a US keyboard.
fn sent_for_mod_shift_backslash() -> Keystroke {
    Keystroke {
        modifiers: secondary(),
        key: "|".into(),
        key_char: None,
    }
}

fn workers_open(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> bool {
    cx.read(|cx| workspace.read(cx).is_workers_open())
}

fn send(cx: &mut TestAppContext, window: AnyWindowHandle, keystroke: Keystroke) {
    act(cx, window, |window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    });
}

#[gpui_kit::test]
fn mod_shift_backslash_as_the_platforms_send_it_toggles_the_workers_panel(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    assert!(!workers_open(&workspace, cx));
    send(cx, window, sent_for_mod_shift_backslash());
    assert!(workers_open(&workspace, cx));
    // One press runs the command once. If both of its bindings ran, the panel
    // would be closed again.
    send(cx, window, sent_for_mod_shift_backslash());
    assert!(!workers_open(&workspace, cx));
}

#[gpui_kit::test]
fn mod_shift_backslash_as_it_reads_toggles_the_workers_panel(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;

    start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.press("secondary-shift-\\", cx)
    });
    assert!(workers_open(&workspace, cx));
    act(cx, window, |window, cx| {
        window.press("secondary-shift-\\", cx)
    });
    assert!(!workers_open(&workspace, cx));
}

#[gpui_kit::test]
fn the_workers_panel_shortcut_reads_as_ctrl_shift_backslash(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    let spelled = Keystroke::parse("secondary-shift-\\").unwrap();
    // The kit's menus and the command palette show the highest precedence
    // binding, which is the last one.
    let shown = cx
        .update_window(window, |_, window, _| {
            let binding = window
                .highest_precedence_binding_for_action(&ToggleWorkersPanel)
                .expect("bound");
            binding.keystrokes()[0].as_keystroke().clone()
        })
        .unwrap();
    assert_eq!(shown, spelled);
    if cfg!(target_os = "macos") {
        assert_eq!(Kbd::format(&shown), "⇧⌘\\");
    } else {
        assert_eq!(Kbd::format(&shown), "Ctrl+Shift+\\");
    }
    // The macOS menu bar takes the first binding.
    let first = cx.read(|cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        keymap
            .bindings_for_action(&ToggleWorkersPanel)
            .next()
            .map(|binding| binding.keystrokes()[0].inner().clone())
    });
    assert_eq!(first, Some(spelled.clone()));
    // The sheet lists the chord once, as it reads.
    let sheet = crate::shortcuts::shortcuts();
    let row = sheet
        .iter()
        .find(|row| row.name == "Toggle Workers Panel")
        .expect("on the sheet");
    assert_eq!(row.keys, [spelled]);
}

/// What the platform's window sends for Mod and `key`, with no Shift held.
fn sent_for_mod(key: &str) -> Keystroke {
    Keystroke {
        modifiers: secondary(),
        key: key.into(),
        key_char: None,
    }
}

/// Zoom In has a binding for `=` and one for `+`, the symbol Ctrl+Shift+= (and
/// the numpad's plus key) arrives as.
#[gpui_kit::test]
fn zoom_follows_the_keystrokes_the_platforms_send_for_plus_and_minus(cx: &mut TestAppContext) {
    let zoom = |cx: &mut TestAppContext| cx.read(|cx| crate::prefs::get(cx).appearance.zoom);
    start(cx);
    let (window, _) = open(cx, 1300.);
    let before = zoom(cx);
    send(cx, window, sent_for_mod("="));
    let once = zoom(cx);
    assert!(once > before);
    send(cx, window, sent_for_mod("+"));
    assert!(zoom(cx) > once, "Mod+Shift+= zooms in");
    send(cx, window, sent_for_mod("-"));
    send(cx, window, sent_for_mod("-"));
    assert!(zoom(cx) < once, "Mod+- zooms out");
    send(cx, window, sent_for_mod("0"));
    assert_eq!(zoom(cx), before);
}

/// The keystrokes of every binding in the keymap, as the platform builds them
/// for this window, with the action and the context each runs in.
fn bindings(cx: &mut TestAppContext) -> Vec<(Keystroke, &'static str, String)> {
    let keymap = cx.read(|cx| cx.key_bindings());
    let keymap = keymap.borrow();
    keymap
        .bindings()
        .map(|binding| {
            (
                binding.keystrokes()[0].inner().clone(),
                binding.action().name(),
                format!("{:?}", binding.predicate()),
            )
        })
        .collect()
}

#[gpui_kit::test]
fn the_delivered_forms_collide_with_no_other_binding(cx: &mut TestAppContext) {
    start(cx);
    let bound = bindings(cx);
    let mut checked = 0;
    for command in commands() {
        for chord in &command.keys {
            let Some(delivered) = delivered_form(chord) else {
                continue;
            };
            let stroke = Keystroke::parse(&delivered).unwrap();
            for (other, action, context) in &bound {
                assert!(
                    other != &stroke || *action == command.action.name(),
                    "{chord} of {} is also bound as {delivered} to {action} ({context})",
                    command.name
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0);
}

/// A binding that spells Shift with a key that carries a symbol never matches
/// a real keystroke, so each needs its delivered form bound to the same action
/// in the same context. This runs over the app's bindings and the text input's
/// tables of every platform.
fn unreachable_spellings(bindings: &[(Keystroke, &'static str, String)]) -> Vec<String> {
    bindings
        .iter()
        .filter(|(stroke, ..)| delivered_form(&stroke.unparse()).is_some())
        .filter(|(stroke, action, context)| {
            let delivered = Keystroke::parse(&delivered_form(&stroke.unparse()).unwrap()).unwrap();
            !bindings.iter().any(|(other, other_action, other_context)| {
                other == &delivered && other_action == action && other_context == context
            })
        })
        .map(|(stroke, action, context)| format!("{} for {action} ({context})", stroke.unparse()))
        .collect()
}

#[gpui_kit::test]
fn no_binding_spells_shift_on_a_symbol_key_without_its_delivered_form(cx: &mut TestAppContext) {
    start(cx);
    let bound = bindings(cx);
    assert!(bound.len() > 100, "only {} bindings", bound.len());
    assert_eq!(unreachable_spellings(&bound), Vec::<String>::new());
}

#[test]
fn the_text_input_tables_of_every_platform_spell_no_shifted_symbols() {
    let mut count = 0;
    for platform in KeymapPlatform::ALL {
        for keymap in Keymap::ALL {
            let bound: Vec<_> = keymap
                .bindings(platform)
                .iter()
                .map(|binding| {
                    (
                        binding.keystrokes()[0].inner().clone(),
                        binding.action().name(),
                        format!("{:?}", binding.predicate()),
                    )
                })
                .collect();
            count += bound.len();
            assert_eq!(
                unreachable_spellings(&bound),
                Vec::<String>::new(),
                "{platform:?} {keymap:?}"
            );
        }
    }
    assert!(count > 300, "only {count} bindings");
}
