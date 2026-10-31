//! Dictation in the composer, with a fake service and microphone. The
//! service's events are given by hand, so nothing needs a model or a
//! microphone.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arko::dictation::models::GRANITE;
use arko::dictation::{
    DictationError, DictationEvent, DictationOptions, DictationUpdate, Preparing, Sink,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::speech::{AudioFormat, AudioInput, AudioSink, SpeechError, SpeechStatus};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Entity, EntityInputHandler as _, Subscription,
    TestAppContext,
};

use crate::Startup;
use crate::backend::FakeBackend;
use crate::composer::{Composer, ComposerEvent};
use crate::dictation::{Dictation, HOLD, Opened, Service, Speech};
use crate::tests::{act, open, press_mouse, row_of};
use crate::workspace::Workspace;

/// What an open dictation of the fake service took.
#[derive(Default)]
struct Taken {
    samples: usize,
    finished: bool,
    dropped: bool,
}

struct FakeOpened(Rc<RefCell<Taken>>);

impl Opened for FakeOpened {
    fn push(&self, samples: &[f32], rate: u32) {
        assert_eq!(rate, 48_000);
        self.0.borrow_mut().samples += samples.len();
    }

    fn finish(&mut self) {
        self.0.borrow_mut().finished = true;
    }
}

impl Drop for FakeOpened {
    fn drop(&mut self) {
        self.0.borrow_mut().dropped = true;
    }
}

/// A dictation the fake service opened.
struct Open {
    download: bool,
    sink: Sink,
    taken: Rc<RefCell<Taken>>,
}

/// The fake service, with whether its model is downloaded and each
/// dictation it opened.
#[derive(Default)]
struct Fake {
    installed: Cell<bool>,
    busy: Cell<bool>,
    opened: RefCell<Vec<Open>>,
}

struct FakeService(Rc<Fake>);

impl Service for FakeService {
    fn open(
        &self,
        options: DictationOptions,
        sink: Sink,
    ) -> Result<Box<dyn Opened>, DictationError> {
        if !options.download && !self.0.installed.get() {
            return Err(DictationError::NeedsDownload(&GRANITE));
        }
        if self.0.busy.get() {
            return Err(DictationError::Busy);
        }
        let taken = Rc::new(RefCell::new(Taken::default()));
        self.0.opened.borrow_mut().push(Open {
            download: options.download,
            sink,
            taken: taken.clone(),
        });
        Ok(Box::new(FakeOpened(taken)))
    }

    fn downloads_without_asking(&self) -> bool {
        false
    }
}

/// A microphone that records while the kit's state has it on.
#[derive(Clone, Default)]
struct FakeMicrophone {
    sink: Rc<RefCell<Option<AudioSink>>>,
    on: Rc<Cell<bool>>,
}

impl AudioInput for FakeMicrophone {
    fn start(
        &self,
        format: AudioFormat,
        sink: AudioSink,
        _: &mut App,
    ) -> Result<Subscription, SpeechError> {
        assert_eq!((format.sample_rate(), format.channels()), (48_000, 1));
        *self.sink.borrow_mut() = Some(sink);
        self.on.set(true);
        let on = self.on.clone();
        Ok(Subscription::new(move || on.set(false)))
    }
}

struct Test {
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    fake: Rc<Fake>,
    microphone: FakeMicrophone,
}

impl Test {
    /// The app with a new session's composer, the fake service (its model
    /// downloaded or not) and the fake microphone.
    fn start(installed: bool, cx: &mut TestAppContext) -> Self {
        let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
        cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
        let fake = Rc::new(Fake::default());
        fake.installed.set(installed);
        let microphone = FakeMicrophone::default();
        cx.update(|cx| {
            cx.set_global(Speech {
                service: Rc::new(FakeService(fake.clone())),
                input: Some(Rc::new(microphone.clone())),
            })
        });
        let (window, workspace) = open(cx, 1300.);
        Self {
            window,
            workspace,
            fake,
            microphone,
        }
    }

    fn composer(&self, cx: &mut TestAppContext) -> Entity<Composer> {
        cx.read(|cx| {
            self.workspace
                .read(cx)
                .session_view()
                .read(cx)
                .composer()
                .clone()
        })
    }

    fn dictation(&self, cx: &mut TestAppContext) -> Entity<Dictation> {
        let composer = self.composer(cx);
        cx.read(|cx| composer.read(cx).dictation().clone())
    }

    fn textarea(&self, cx: &mut TestAppContext) -> Entity<TextareaState> {
        let composer = self.composer(cx);
        cx.read(|cx| composer.read(cx).textarea().clone())
    }

    fn text(&self, cx: &mut TestAppContext) -> String {
        let composer = self.composer(cx);
        cx.read(|cx| composer.read(cx).text(cx))
    }

    fn status(&self, cx: &mut TestAppContext) -> SpeechStatus {
        let dictation = self.dictation(cx);
        cx.read(|cx| dictation.read(cx).speech().read(cx).status())
    }

    /// What shows beside the button.
    fn words(&self, cx: &mut TestAppContext) -> Option<String> {
        let dictation = self.dictation(cx);
        let words = cx.read(|cx| dictation.read(cx).words().map(|words| words.to_string()));
        act(cx, self.window, |window, _| {
            assert_eq!(
                window.try_find("dictation-words").is_some(),
                words.is_some()
            );
        });
        words
    }

    fn type_text(&self, text: &str, cx: &mut TestAppContext) {
        act(cx, self.window, |window, cx| window.input(text, cx));
    }

    fn click(&self, cx: &mut TestAppContext) {
        press_mouse(cx, self.window, "dictate", true);
        press_mouse(cx, self.window, "dictate", false);
    }

    /// Dictation `index` of the fake service hears `event`.
    fn hear(&self, index: usize, event: DictationEvent, cx: &mut TestAppContext) {
        (self.fake.opened.borrow_mut()[index].sink)(DictationUpdate {
            id: index as u64 + 1,
            event,
        });
        cx.run_until_parked();
    }

    fn taken(&self, index: usize) -> Rc<RefCell<Taken>> {
        self.fake.opened.borrow()[index].taken.clone()
    }

    /// A recording that listens, as dictation 0.
    fn listening(&self, cx: &mut TestAppContext) {
        self.click(cx);
        assert_eq!(self.status(cx), SpeechStatus::Connecting);
        assert!(self.microphone.on.get());
        self.hear(0, DictationEvent::Preparing(Preparing::Loading), cx);
        assert_eq!(self.words(cx).as_deref(), Some("Loading speech model"));
        self.hear(0, DictationEvent::Listening, cx);
        assert_eq!(self.status(cx), SpeechStatus::Recording);
        assert_eq!(self.words(cx), None);
    }

    fn notifications(&self, cx: &mut TestAppContext) -> usize {
        cx.update_window(self.window, |_, window, cx| window.notifications(cx).len())
            .unwrap()
    }

    /// Another recording that listens, and its dictation's index.
    fn record(&self, cx: &mut TestAppContext) -> usize {
        self.click(cx);
        let index = self.fake.opened.borrow().len() - 1;
        self.hear(index, DictationEvent::Listening, cx);
        assert_eq!(self.status(cx), SpeechStatus::Recording);
        index
    }
}

fn text(settled: &str, tentative: &str) -> DictationEvent {
    DictationEvent::Text {
        settled: settled.into(),
        tentative: tentative.into(),
    }
}

/// A click records until the next click. The microphone's audio goes to
/// the service at its rate, settled words go in as they come, the tentative
/// ones show beside the button, and the waveform shows while it listens.
/// The second click stops the microphone and lets the last words come.
#[gpui_kit::test]
fn a_click_records_until_the_next_click(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.listening(cx);
    act(cx, test.window, |window, _| {
        assert_eq!(window.find("dictation-waveform").label(), Some("Listening"));
    });
    let audio = test.microphone.sink.borrow().clone().unwrap();
    cx.update(|cx| audio.push(vec![1_000; 960], cx));
    cx.run_until_parked();
    assert_eq!(test.taken(0).borrow().samples, 960);

    test.hear(0, text("", "Hello wor"), cx);
    assert_eq!(test.text(cx), "");
    assert_eq!(test.words(cx).as_deref(), Some("Hello wor"));
    test.hear(0, text("Hello", " world"), cx);
    assert_eq!(test.text(cx), "Hello");
    assert_eq!(test.words(cx).as_deref(), Some("world"));

    test.click(cx);
    assert_eq!(test.status(cx), SpeechStatus::Stopping);
    assert!(!test.microphone.on.get());
    assert!(test.taken(0).borrow().finished);
    act(cx, test.window, |window, _| {
        assert!(window.try_find("dictation-waveform").is_none());
    });
    test.hear(0, text(" world.", ""), cx);
    test.hear(0, DictationEvent::Done, cx);
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(test.taken(0).borrow().dropped);
    assert_eq!(test.text(cx), "Hello world.");
    assert_eq!(test.words(cx), None);
    // The words are the user's now, and Esc leaves them.
    act(cx, test.window, |window, cx| window.press("escape", cx));
    assert_eq!(test.text(cx), "Hello world.");
}

/// A press held past the hold time records until it's released, and a
/// shorter one keeps recording.
#[gpui_kit::test]
fn a_held_press_records_until_released(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    press_mouse(cx, test.window, "dictate", true);
    assert_eq!(test.status(cx), SpeechStatus::Connecting, "starts on press");
    cx.executor()
        .advance_clock(HOLD + Duration::from_millis(50));
    cx.run_until_parked();
    test.hear(0, DictationEvent::Listening, cx);
    test.hear(0, text("Hold", ""), cx);
    assert_eq!(test.status(cx), SpeechStatus::Recording);
    press_mouse(cx, test.window, "dictate", false);
    assert_eq!(test.status(cx), SpeechStatus::Stopping);
    assert!(test.taken(0).borrow().finished);
    test.hear(0, DictationEvent::Done, cx);
    assert_eq!(test.text(cx), "Hold");

    // A short press is a click.
    press_mouse(cx, test.window, "dictate", true);
    cx.executor()
        .advance_clock(HOLD - Duration::from_millis(100));
    press_mouse(cx, test.window, "dictate", false);
    cx.executor().advance_clock(HOLD);
    cx.run_until_parked();
    assert_eq!(test.status(cx), SpeechStatus::Connecting);
    assert!(!test.taken(1).borrow().finished);
}

/// Words go in at the caret, with a space after a word, and the caret
/// follows them.
#[gpui_kit::test]
fn words_go_in_at_the_caret(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.type_text("Hello world", cx);
    let textarea = test.textarea(cx);
    textarea.update(cx, |textarea, cx| textarea.set_selected_range(5..5, cx));
    test.listening(cx);
    test.hear(0, text("big", ""), cx);
    assert_eq!(test.text(cx), "Hello big world");
    assert_eq!(cx.read(|cx| textarea.read(cx).cursor()), "Hello big".len());
    // The next words follow the first.
    test.hear(0, text(" round", ""), cx);
    assert_eq!(test.text(cx), "Hello big round world");
}

/// Esc cancels. The recording ends at once, and the words it put in leave
/// the box, wherever the caret took them. What was there before and what
/// the user typed meanwhile stay, even typed alike.
#[gpui_kit::test]
fn esc_takes_out_only_this_recordings_words(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.type_text("Note:", cx);
    test.listening(cx);
    test.hear(0, text("one two", " three"), cx);
    assert_eq!(test.text(cx), "Note: one two");
    act(cx, test.window, |window, cx| window.press("escape", cx));
    assert_eq!(test.text(cx), "Note:");
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(!test.microphone.on.get());
    assert!(test.taken(0).borrow().dropped);
    assert_eq!(test.words(cx), None);

    // Typed beside the recording's words, and alike.
    act(cx, test.window, |window, cx| window.press("backspace", cx));
    test.type_text(" ", cx);
    test.click(cx);
    test.hear(1, DictationEvent::Listening, cx);
    test.hear(1, text("cat", ""), cx);
    assert_eq!(test.text(cx), "Note cat");
    let textarea = test.textarea(cx);
    textarea.update(cx, |textarea, cx| textarea.set_selected_range(5..5, cx));
    test.type_text("cat dog ", cx);
    test.hear(1, text(" ears", ""), cx);
    assert_eq!(test.text(cx), "Note cat dog  earscat");
    act(cx, test.window, |window, cx| window.press("escape", cx));
    assert_eq!(test.text(cx), "Note cat dog ");
}

/// The send key while recording stops it rather than sending, and sends
/// once the words are in. A recording never sends by itself.
#[gpui_kit::test]
fn the_send_key_stops_a_recording_and_sends_after(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    let composer = test.composer(cx);
    let sent = Rc::new(Cell::new(0));
    let _counting = cx.update(|cx| {
        let sent = sent.clone();
        cx.subscribe(&composer, move |_, event: &ComposerEvent, _| {
            if matches!(event, ComposerEvent::Submit(_)) {
                sent.set(sent.get() + 1);
            }
        })
    });
    test.listening(cx);
    test.hear(0, text("Ship it", ""), cx);
    act(cx, test.window, |window, cx| window.press("enter", cx));
    assert_eq!(test.status(cx), SpeechStatus::Stopping);
    // Again while the last words come, and it waits for them.
    act(cx, test.window, |window, cx| window.press("enter", cx));
    assert_eq!(sent.get(), 0);
    test.hear(0, text(".", ""), cx);
    test.hear(0, DictationEvent::Done, cx);
    assert_eq!((test.text(cx).as_str(), sent.get()), ("Ship it.", 0));
    act(cx, test.window, |window, cx| window.press("enter", cx));
    assert_eq!(sent.get(), 1);
}

/// Showing another session ends the recording. Its words stay in the draft
/// of the session they were for, and the rest of it goes nowhere.
#[gpui_kit::test]
fn another_session_ends_the_recording(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.listening(cx);
    test.hear(0, text("Half a thought", ""), cx);
    let row = row_of(&test.workspace, "Split the engine out of main.rs", cx);
    act(cx, test.window, |window, cx| window.click(("row", row), cx));
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(test.taken(0).borrow().dropped);
    assert!(!test.microphone.on.get());
    assert_eq!(test.text(cx), "");
    assert_eq!(
        cx.read(|cx| crate::drafts::get(crate::drafts::NEW_SESSION, cx))
            .map(|draft| draft.text)
            .as_deref(),
        Some("Half a thought")
    );
    test.hear(0, text(" more", ""), cx);
    assert_eq!(test.text(cx), "");
}

/// While an input method composes, settled words wait, and go in once it's
/// done.
#[gpui_kit::test]
fn words_wait_for_an_input_method(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    let index = test.record(cx);
    let textarea = test.textarea(cx);
    act(cx, test.window, |window, cx| {
        textarea.update(cx, |textarea, cx| {
            textarea.replace_and_mark_text_in_range(None, "に", None, window, cx)
        })
    });
    test.hear(index, text("Hello", ""), cx);
    assert_eq!(test.text(cx), "に");
    act(cx, test.window, |window, cx| {
        textarea.update(cx, |textarea, cx| {
            textarea.replace_text_in_range(None, "日", window, cx)
        })
    });
    assert_eq!(test.text(cx), "日 Hello");
}

/// When the last words take too long, the recording ends with the words so
/// far, and says so.
#[gpui_kit::test]
fn last_words_that_take_too_long_leave_the_rest(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.listening(cx);
    test.hear(0, text("Half", ""), cx);
    let notes = test.notifications(cx);
    test.click(cx);
    assert_eq!(test.status(cx), SpeechStatus::Stopping);
    cx.executor().advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(test.taken(0).borrow().dropped);
    assert_eq!(test.text(cx), "Half");
    assert_eq!(test.notifications(cx), notes + 1);
}

/// The first recording without a model asks before downloading it, and
/// Cancel leaves it. Download runs beside the button with the service's
/// steps, without the microphone, and says when the model is ready. The
/// download can be stopped.
#[gpui_kit::test]
fn the_first_recording_asks_before_downloading_the_model(cx: &mut TestAppContext) {
    let test = Test::start(false, cx);
    test.click(cx);
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(!test.microphone.on.get());
    act(cx, test.window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        window.within("dialog").click("cancel", cx)
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    act(cx, test.window, |window, cx| {
        assert!(!window.has_active_dialog(cx))
    });
    assert!(test.fake.opened.borrow().is_empty());

    test.click(cx);
    act(cx, test.window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    assert_eq!(test.fake.opened.borrow().len(), 1);
    assert!(test.fake.opened.borrow()[0].download, "it downloads");
    assert!(!test.microphone.on.get());
    assert_eq!(
        test.words(cx).as_deref(),
        Some("Downloading speech model (0 of 948 MB)")
    );
    act(cx, test.window, |window, _| {
        assert!(window.try_find("dictate").is_none());
        assert!(window.try_find("cancel-download").is_some());
    });
    test.hear(
        0,
        DictationEvent::Preparing(Preparing::Downloading {
            done: 412_000_000,
            total: GRANITE.download_bytes(),
        }),
        cx,
    );
    assert_eq!(
        test.words(cx).as_deref(),
        Some("Downloading speech model (412 of 948 MB)")
    );
    test.hear(0, DictationEvent::Preparing(Preparing::Packing), cx);
    assert_eq!(test.words(cx).as_deref(), Some("Preparing speech model"));
    test.hear(0, DictationEvent::Preparing(Preparing::Loading), cx);
    let notes = test.notifications(cx);
    test.hear(0, DictationEvent::Listening, cx);
    assert!(test.taken(0).borrow().dropped);
    assert_eq!(test.notifications(cx), notes + 1);
    assert_eq!(test.words(cx), None);
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(!test.microphone.on.get());

    // Cancel download.
    test.click(cx);
    act(cx, test.window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    act(cx, test.window, |window, cx| {
        window.click("cancel-download", cx)
    });
    assert!(test.taken(1).borrow().dropped);
    assert_eq!(test.words(cx), None);
    act(cx, test.window, |window, _| {
        assert!(window.try_find("dictate").is_some());
    });
}

/// Busy and a recording that fails say so in a notification. The words
/// that went in stay.
#[gpui_kit::test]
fn busy_and_failures_say_so(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.fake.busy.set(true);
    test.click(cx);
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(!test.microphone.on.get());
    assert_eq!(test.notifications(cx), 1);

    test.fake.busy.set(false);
    test.listening(cx);
    test.hear(0, text("Half", ""), cx);
    test.hear(
        0,
        DictationEvent::Failed("The microphone stopped (gone).".into()),
        cx,
    );
    assert_eq!(test.status(cx), SpeechStatus::Idle);
    assert!(!test.microphone.on.get());
    assert_eq!(test.text(cx), "Half");
    assert_eq!(test.notifications(cx), 2);
}

/// A WAV file's mono 16-bit samples, at three times its rate (16 kHz to
/// the 48 kHz the recognizer asks for), each sample repeated.
fn wav_at_48_khz(path: &std::path::Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    let mut at = 12;
    let data = loop {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            break &bytes[at + 8..at + 8 + size];
        }
        at += 8 + size + size % 2;
    };
    data.chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .flat_map(|sample| [sample; 3])
        .collect()
}

/// A microphone that hears a WAV file and then 3 s of silence, in 20 ms
/// pieces.
struct WavMicrophone(Vec<i16>);

impl AudioInput for WavMicrophone {
    fn start(
        &self,
        _: AudioFormat,
        sink: AudioSink,
        cx: &mut App,
    ) -> Result<Subscription, SpeechError> {
        let silence = vec![0; 48_000 * 3];
        for piece in self.0.chunks(960).chain(silence.chunks(960)) {
            sink.push(piece.to_vec(), cx);
        }
        Ok(Subscription::new(|| {}))
    }
}

/// A WAV file dictated through the recognizer, Arko's service and Granite
/// Speech's real pack, without a microphone. The model loads, the words go
/// into the box as they settle, and a click lets the last ones come. It runs
/// only with `ARKO_TEST_SPEECH_PACK` naming Granite's int8 pack and
/// `ARKO_TEST_SPEECH_WAV` LibriSpeech's clip 260-123286-0018, as Arko's own
/// test does.
#[gpui_kit::test]
fn a_wav_file_is_dictated_through_arkos_service(cx: &mut TestAppContext) {
    let (Some(pack), Some(wav)) = (
        std::env::var_os("ARKO_TEST_SPEECH_PACK"),
        std::env::var_os("ARKO_TEST_SPEECH_WAV"),
    ) else {
        eprintln!("skipped: ARKO_TEST_SPEECH_PACK and ARKO_TEST_SPEECH_WAV aren't set");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(arko::dictation::settings::FILE),
        r#"{"model": "granite"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(GRANITE.dir(root.path())).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&pack, GRANITE.pack(root.path())).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(&pack, GRANITE.pack(root.path())).unwrap();
    // The service's thread wakes the app's tasks, which the test scheduler
    // allows as for real I/O.
    cx.executor().allow_parking();
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| {
        crate::init(Startup::for_tests(backend), cx);
        crate::dictation::init(root.path().to_path_buf(), cx);
        let mut speech = cx.global::<Speech>().clone();
        speech.input = Some(Rc::new(WavMicrophone(wav_at_48_khz(wav.as_ref()))));
        cx.set_global(speech);
    });
    let (window, workspace) = open(cx, 1300.);
    let test = Test {
        window,
        workspace,
        fake: Rc::default(),
        microphone: FakeMicrophone::default(),
    };
    test.type_text("Notes:", cx);
    let started = std::time::Instant::now();
    test.click(cx);
    crate::testing::wait_until(cx, "the model to listen", |cx| {
        test.status(cx) == SpeechStatus::Recording
    });
    eprintln!("listening after {:?}", started.elapsed());
    crate::testing::wait_until(cx, "the words", |cx| test.text(cx).contains("length"));
    eprintln!("words after {:?}: {:?}", started.elapsed(), test.text(cx));
    test.click(cx);
    crate::testing::wait_until(cx, "the last words", |cx| {
        test.status(cx) == SpeechStatus::Idle
    });
    let text = test.text(cx);
    eprintln!("done after {:?}: {text:?}", started.elapsed());
    assert!(text.starts_with("Notes: "), "{text:?}");
    let lower = text.to_lowercase();
    for word in ["i saw", "hamburg", "skeleton", "thirty feet in length"] {
        assert!(lower.contains(word), "{text:?}");
    }
}

/// Listening draws the box's row and the views around it, and leaves the
/// other panes as they were. Without motion the waveform draws once per
/// level (80 ms of audio). With motion the kit's waveform asks for every
/// frame.
#[gpui_kit::test]
fn listening_draws_no_other_pane(cx: &mut TestAppContext) {
    let test = Test::start(true, cx);
    test.listening(cx);
    let audio = test.microphone.sink.borrow().clone().unwrap();
    let view = cx.read(|cx| test.workspace.read(cx).session_view().clone());
    for motion in [false, true] {
        cx.update(|cx| cx.set_reduce_motion(!motion));
        let before = cx.read(|cx| view.read(cx).render_count());
        crate::testing::count_renders();
        // A second of a 60 Hz display, and 12 levels of audio.
        for frame in 0..60 {
            if frame % 5 == 0 {
                cx.update(|cx| audio.push(vec![8_000; 3_840], cx));
            }
            cx.executor().advance_clock(Duration::from_micros(16_667));
            cx.update_window(test.window, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
        let drawn = cx.read(|cx| view.read(cx).render_count()) - before;
        let others = crate::testing::renders();
        assert!(others.keys().all(|view| *view == "Workspace"), "{others:?}");
        if motion {
            assert!(drawn <= 75, "{drawn}");
        } else {
            assert!(drawn <= 13, "{drawn}");
        }
    }
}
