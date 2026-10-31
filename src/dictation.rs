//! Dictation, speech typed into the message box and recognized on this
//! computer by Arko's dictation service.
//!
//! The kit's [`SpeechState`] runs each recording, with its [`SpeechButton`]
//! and [`SpeechWaveform`], and captures from the kit's microphone. Its
//! recognizer is Arko's service ([`ArkoRecognizer`]), whose settled words
//! become the kit's committed phrases and whose tentative ones become its
//! hypothesis. Settled words go into the box at the caret as they come, and
//! the tentative ones show muted beside the button. Esc cancels, and takes
//! out the words the recording put in that are still there, followed through
//! the user's edits. A recording never sends.
//!
//! The button starts recording on press. A short click keeps it recording
//! until the next click, and a press held past [`HOLD`] records until it's
//! released. The first recording without a model asks before downloading
//! it, and the download runs beside the button without the microphone.

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use arko::dictation::{
    CANCEL, DOWNLOAD, DOWNLOAD_TITLE, DictationError, DictationEvent, DictationOptions,
    DictationService, Preparing, Sink, SpeechModel,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::speech::{
    AudioFormat, AudioInput, RecognitionSession, SpeechButton, SpeechError, SpeechEvent,
    SpeechRecognizer, SpeechSink, SpeechState, SpeechWaveform,
};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tokio::sync::mpsc;

use crate::workers::curly;

/// A press on the button held this long records until it's released.
pub const HOLD: Duration = Duration::from_millis(300);

/// The rate the recognizer asks the kit's microphone for. It's most
/// devices' own, so the kit passes their audio through and Arko's resampler
/// converts it.
const RATE: u32 = 48_000;

/// How long the kit waits for the last words once recording stops, a guess.
/// Arko's last pass covers the audio recognition hadn't caught up on, at
/// most 30 s of it, so a computer that kept up while listening takes far
/// less.
const LAST_WORDS: Duration = Duration::from_secs(30);

/// What the window says once the model is downloaded and ready.
const MODEL_READY: &str = "Speech model downloaded.";

/// What the window says when the last words took longer than
/// [`LAST_WORDS`].
const TOO_SLOW: &str = "Dictation took too long to finish. The words so far stay in the box.";

/// What macOS's refusal of the microphone says.
const MICROPHONE_DENIED: &str = "Arko Studio isn’t allowed to use the microphone. Allow it in System Settings › Privacy & Security › Microphone.";

/// Arko's dictation service as recording uses it, the real one or a fake in
/// tests.
pub trait Service: 'static {
    /// Opens a dictation whose events go to `sink`. A model that isn't
    /// downloaded is [`DictationError::NeedsDownload`], unless
    /// `options.download`.
    fn open(
        &self,
        options: DictationOptions,
        sink: Sink,
    ) -> Result<Box<dyn Opened>, DictationError>;

    /// Whether Arko's settings download the model without asking.
    fn downloads_without_asking(&self) -> bool;
}

/// An open dictation, which takes the audio. Dropping it cancels it.
pub trait Opened {
    /// Mono audio at `rate` samples a second.
    fn push(&self, samples: &[f32], rate: u32);
    /// Ends the audio. The last words settle, then `Done` comes.
    fn finish(&mut self);
}

impl Opened for arko::dictation::Dictation {
    fn push(&self, samples: &[f32], rate: u32) {
        arko::dictation::Dictation::push(self, samples, rate);
    }

    fn finish(&mut self) {
        arko::dictation::Dictation::finish(self);
    }
}

/// Arko's dictation service for Arko's folder.
struct Arko {
    root: PathBuf,
}

impl Service for Arko {
    fn open(
        &self,
        options: DictationOptions,
        sink: Sink,
    ) -> Result<Box<dyn Opened>, DictationError> {
        let service = DictationService::shared(&self.root);
        // A missing model downloads on its own, never inside a recording, so
        // the microphone never runs through a download. Whether that asks
        // first is up to `Dictation::offer_download`.
        let model = service.model();
        if !options.download && !model.is_installed(&self.root) {
            return Err(DictationError::NeedsDownload(model));
        }
        Ok(Box::new(service.open(options, sink)?))
    }

    fn downloads_without_asking(&self) -> bool {
        arko::dictation::settings::Settings::load(&self.root).downloads_without_asking()
    }
}

/// What recordings use, Arko's service and the audio a test gives (the
/// kit's microphone otherwise).
#[derive(Clone)]
pub struct Speech {
    pub service: Rc<dyn Service>,
    pub input: Option<Rc<dyn AudioInput>>,
}

impl Global for Speech {}

/// Recordings use Arko's service for Arko's folder `root`.
pub fn init(root: PathBuf, cx: &mut App) {
    cx.set_global(Speech {
        service: Rc::new(Arko { root }),
        input: None,
    });
}

/// What every dictation opens with. `download` says the user agreed to
/// download the model.
fn options(download: bool) -> DictationOptions {
    // Arko 0.17.0's `arko::dictation::session_words`, the session's names
    // for the engine to favour, go in here once Studio builds on it.
    DictationOptions { download }
}

/// The kit's recognizer on Arko's dictation service. Each event goes to the
/// kit's state and to the [`Dictation`] whose recording it is.
struct ArkoRecognizer {
    owner: WeakEntity<Dictation>,
    window: AnyWindowHandle,
}

impl SpeechRecognizer for ArkoRecognizer {
    fn audio_format(&self) -> AudioFormat {
        AudioFormat::new(RATE, 1)
    }

    fn start(
        &self,
        sink: SpeechSink,
        cx: &mut App,
    ) -> Result<Box<dyn RecognitionSession>, SpeechError> {
        let (sender, mut updates) = mpsc::unbounded_channel();
        let service = cx.global::<Speech>().service.clone();
        let opened = service
            .open(
                options(false),
                Box::new(move |update| {
                    let _ = sender.send(update.event);
                }),
            )
            .map_err(SpeechError::recognizer)?;
        let (owner, window) = (self.owner.clone(), self.window);
        let events = cx.spawn(async move |cx| {
            while let Some(event) = updates.recv().await {
                cx.update(|cx| {
                    match &event {
                        DictationEvent::Preparing(_) => {}
                        DictationEvent::Listening => sink.ready(cx),
                        DictationEvent::Text { settled, tentative } => {
                            if !settled.is_empty() {
                                sink.phrase(settled.clone(), cx);
                            }
                            sink.hypothesis(tentative.clone(), cx);
                        }
                        DictationEvent::Done => sink.finish(cx),
                        DictationEvent::Failed(why) => {
                            sink.error(SpeechError::recognizer(anyhow::Error::msg(why.clone())), cx)
                        }
                    }
                    window
                        .update(cx, |_, window, cx| {
                            owner.update(cx, |dictation, cx| dictation.heard(&event, window, cx))
                        })
                        .ok();
                });
            }
        });
        Ok(Box::new(Session {
            opened,
            samples: Vec::new(),
            _events: events,
        }))
    }
}

/// A recording's dictation, while the kit's state runs it.
struct Session {
    opened: Box<dyn Opened>,
    samples: Vec<f32>,
    _events: Task<()>,
}

impl RecognitionSession for Session {
    fn push_audio(&mut self, samples: &[i16], _: &mut App) {
        self.samples.clear();
        self.samples.extend(
            samples
                .iter()
                .map(|&sample| f32::from(sample) / f32::from(i16::MAX)),
        );
        self.opened.push(&self.samples, RATE);
    }

    fn finish(&mut self, _: &mut App) {
        self.opened.finish();
    }
}

/// The recording that's on.
struct Recording {
    /// The parts of the box its words went into, as the user's edits since
    /// moved them.
    ranges: Vec<Range<usize>>,
    /// The box's text as last seen, to find each edit.
    seen: String,
    /// Some of its words went in.
    wrote: bool,
    /// The words after them that may still change.
    tentative: SharedString,
    /// Making the model ready, before it listens.
    preparing: Option<Preparing>,
    /// Arko said its last words are in.
    done: bool,
}

/// The model downloading for the recordings to come. Dropping it stops the
/// download, and the next one resumes it.
struct Download {
    _opened: Box<dyn Opened>,
    step: Preparing,
    _events: Task<()>,
}

/// A press on the button that started a recording.
struct Press {
    /// Held past [`HOLD`], so the release stops the recording.
    held: bool,
    _timer: Task<()>,
}

/// The composer's dictation, with the recording, the button and what shows
/// beside it.
pub struct Dictation {
    speech: Entity<SpeechState>,
    textarea: Entity<TextareaState>,
    recording: Option<Recording>,
    /// Settled words that wait for an input method to finish composing.
    held: String,
    download: Option<Download>,
    press: Option<Press>,
    _subscriptions: Vec<Subscription>,
}

impl Dictation {
    pub fn new(
        textarea: Entity<TextareaState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let recognizer = ArkoRecognizer {
            owner: cx.weak_entity(),
            window: window.window_handle(),
        };
        let input = cx
            .try_global::<Speech>()
            .and_then(|speech| speech.input.clone());
        let speech = cx.new(|cx| {
            let state = SpeechState::new(cx)
                .recognizer(recognizer)
                // Windows' own recognizer sends the audio to Microsoft.
                .system_fallback(false)
                .stop_timeout(LAST_WORDS);
            match input {
                Some(input) => state.input(input),
                None => state,
            }
        });
        let subscriptions = vec![
            cx.subscribe_in(&speech, window, Self::on_speech_event),
            cx.subscribe_in(
                &textarea,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.follow_edits(cx);
                        this.write_held(window, cx);
                    }
                },
            ),
            // The button and the waveform follow the state.
            cx.observe(&speech, |_, _, cx| cx.notify()),
        ];
        Self {
            speech,
            textarea,
            recording: None,
            held: String::new(),
            download: None,
            press: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn speech(&self) -> &Entity<SpeechState> {
        &self.speech
    }

    /// Whether a recording is on, from its start until its last words are in.
    fn is_recording(&self, cx: &App) -> bool {
        self.speech.read(cx).status().is_active()
    }

    /// Starts recording, unless one runs or the model downloads.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.download.is_none() {
            self.speech.update(cx, |speech, cx| speech.start(cx));
        }
    }

    /// Stops recording, and lets the last words come. Whether a recording
    /// was on, so the send key waits for them rather than sending.
    pub fn stop(&mut self, cx: &mut Context<Self>) -> bool {
        self.press = None;
        let on = self.is_recording(cx);
        self.speech.update(cx, |speech, cx| speech.stop(cx));
        on
    }

    /// Esc while recording. It ends the recording and takes its words out of
    /// the box. Whether a recording was on.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.is_recording(cx) {
            return false;
        }
        self.press = None;
        self.held.clear();
        self.follow_edits(cx);
        if let Some(recording) = self.recording.take() {
            self.textarea.update(cx, |textarea, cx| {
                for range in recording.ranges.into_iter().rev() {
                    textarea.set_selected_range(range, cx);
                    textarea.replace("", window, cx);
                }
            });
        }
        self.speech.update(cx, |speech, cx| speech.cancel(cx));
        cx.notify();
        true
    }

    /// Ends the recording where it is, as the box goes to another session.
    /// The words in it stay, and those still to come don't.
    pub fn end(&mut self, cx: &mut Context<Self>) {
        self.recording = None;
        self.held.clear();
        self.press = None;
        self.speech.update(cx, |speech, cx| speech.cancel(cx));
    }

    /// Whether an input method composes text in the box.
    fn is_composing(&self, window: &mut Window, cx: &mut App) -> bool {
        self.textarea.update(cx, |textarea, cx| {
            textarea.marked_text_range(window, cx).is_some()
        })
    }

    /// The button pressed. It starts a recording, or stops the one on.
    fn press(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let speech = self.speech.read(cx);
        let (status, available) = (speech.status(), speech.is_available(cx));
        if status.is_capturing() {
            self.stop(cx);
            return;
        }
        if status.is_active() || !available {
            return;
        }
        // The words go where the caret is, and Esc reaches the box.
        self.textarea
            .update(cx, |textarea, cx| textarea.focus(window, cx));
        self.start(cx);
        if !self.is_recording(cx) {
            return;
        }
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOLD).await;
            this.update(cx, |this, _| {
                if let Some(press) = &mut this.press {
                    press.held = true;
                }
            })
            .ok();
        });
        self.press = Some(Press {
            held: false,
            _timer: timer,
        });
    }

    /// The button released. A press held past [`HOLD`] stops the recording
    /// it started, and a shorter one leaves it on.
    fn release(&mut self, cx: &mut Context<Self>) {
        if self.press.take().is_some_and(|press| press.held) {
            self.stop(cx);
        }
    }

    fn on_speech_event(
        &mut self,
        _: &Entity<SpeechState>,
        event: &SpeechEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SpeechEvent::Started => {
                self.recording = Some(Recording {
                    ranges: Vec::new(),
                    seen: self.textarea.read(cx).text().to_string(),
                    wrote: false,
                    tentative: SharedString::default(),
                    preparing: None,
                    done: false,
                });
            }
            SpeechEvent::Partial(_) => {}
            // Its words are the user's now. The kit ends a recording whose
            // last words didn't come in time as if they had.
            SpeechEvent::Final(_) => {
                let cut_short = self
                    .recording
                    .take()
                    .is_some_and(|recording| !recording.done);
                self.press = None;
                if cut_short {
                    window.push_notification(Notification::warning(TOO_SLOW), cx);
                }
            }
            SpeechEvent::Cancelled => {
                self.recording = None;
                self.press = None;
            }
            SpeechEvent::Error(error) => {
                let started = self.recording.take().is_some();
                self.press = None;
                self.failed(error, started, window, cx);
            }
        }
        cx.notify();
    }

    /// An event of the recording's dictation, after the kit's state had it.
    fn heard(&mut self, event: &DictationEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_none() {
            return;
        }
        match event {
            DictationEvent::Preparing(step) => {
                if let Some(recording) = &mut self.recording {
                    recording.preparing = Some(*step);
                }
            }
            DictationEvent::Listening => {
                if let Some(recording) = &mut self.recording {
                    recording.preparing = None;
                }
            }
            DictationEvent::Text { settled, tentative } => {
                self.write(settled, window, cx);
                if let Some(recording) = &mut self.recording {
                    recording.tentative = tentative.trim().to_string().into();
                }
            }
            DictationEvent::Done => {
                if let Some(recording) = &mut self.recording {
                    recording.done = true;
                }
            }
            DictationEvent::Failed(_) => {}
        }
        cx.notify();
    }

    /// Settled words go in at the caret, wherever it is. While an input
    /// method composes they wait, and go in once it's done.
    fn write(&mut self, settled: &str, window: &mut Window, cx: &mut Context<Self>) {
        if settled.is_empty() {
            return;
        }
        if self.is_composing(window, cx) {
            self.held.push_str(settled);
            return;
        }
        self.follow_edits(cx);
        let wrote = self
            .recording
            .as_ref()
            .is_none_or(|recording| recording.wrote);
        let (range, text) = self.textarea.update(cx, |textarea, cx| {
            let at = textarea.cursor();
            let text = textarea.text().to_string();
            let words = spaced(text[..at].chars().next_back(), wrote, settled);
            textarea.insert(words.clone(), window, cx);
            (at..at + words.len(), textarea.text().to_string())
        });
        let Some(recording) = &mut self.recording else {
            return;
        };
        match recording.ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => recording.ranges.push(range),
        }
        recording.seen = text;
        recording.wrote = true;
    }

    /// The words held while an input method composed, once it's done.
    fn write_held(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.held.is_empty() || self.is_composing(window, cx) {
            return;
        }
        let held = std::mem::take(&mut self.held);
        self.write(&held, window, cx);
    }

    /// Moves the recording's ranges with what changed in the box since it
    /// last looked.
    fn follow_edits(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        let textarea = self.textarea.read(cx);
        let text = textarea.text().to_string();
        if text != recording.seen {
            let (replaced, inserted) = edit_between(&recording.seen, &text, textarea.cursor());
            follow(&mut recording.ranges, replaced, inserted);
            recording.seen = text;
        }
    }

    /// Says why a recording ended or couldn't start, in Arko's words. A
    /// missing model asks to download it.
    fn failed(
        &mut self,
        error: &SpeechError,
        started: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let words = match error {
            SpeechError::Recognizer(error) => match error.downcast_ref::<DictationError>() {
                Some(DictationError::NeedsDownload(model)) => {
                    return self.offer_download(model, window, cx);
                }
                Some(busy @ DictationError::Busy) => {
                    window.push_notification(curly(&busy.to_string()), cx);
                    return;
                }
                Some(other) => other.to_string(),
                None => error.to_string(),
            },
            SpeechError::NoInputDevice => "No microphone found.".into(),
            SpeechError::PermissionDenied => MICROPHONE_DENIED.into(),
            SpeechError::Input(error) if started => format!("The microphone stopped ({error})."),
            SpeechError::Input(error) => format!("Couldn't open the microphone ({error})."),
            SpeechError::Unsupported => DictationError::Unavailable.to_string(),
        };
        window.push_notification(Notification::error(curly(&words)), cx);
    }

    /// Asks before downloading `model`, unless Arko's settings download it
    /// without asking.
    fn offer_download(
        &mut self,
        model: &'static SpeechModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<Speech>().service.downloads_without_asking() {
            return self.download(model, window, cx);
        }
        let this = cx.weak_entity();
        let offer = curly(&model.offer());
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(DOWNLOAD_TITLE)
                .description(offer.clone())
                .confirm()
                .ok_text(DOWNLOAD)
                .cancel_text(CANCEL)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.download(model, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// Downloads `model` beside the box, then loads it, without recording.
    fn download(
        &mut self,
        model: &'static SpeechModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.download.is_some() {
            return;
        }
        let (sender, mut updates) = mpsc::unbounded_channel();
        let service = cx.global::<Speech>().service.clone();
        let sink: Sink = Box::new(move |update| {
            let _ = sender.send(update.event);
        });
        let opened = match service.open(options(true), sink) {
            Ok(opened) => opened,
            Err(error) => {
                let error = SpeechError::recognizer(error);
                return self.failed(&error, false, window, cx);
            }
        };
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = updates.recv().await {
                let went = this.update_in(cx, |this, window, cx| {
                    this.download_event(event, window, cx)
                });
                if went.is_err() {
                    return;
                }
            }
        });
        self.download = Some(Download {
            _opened: opened,
            step: Preparing::Downloading {
                done: 0,
                total: model.download_bytes(),
            },
            _events: events,
        });
        cx.notify();
    }

    fn download_event(
        &mut self,
        event: DictationEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DictationEvent::Preparing(step) => {
                if let Some(download) = &mut self.download {
                    download.step = step;
                }
            }
            // The model is ready. It stays loaded a while, and the
            // microphone waits for the button.
            DictationEvent::Listening | DictationEvent::Done => {
                self.download = None;
                window.push_notification(MODEL_READY, cx);
            }
            DictationEvent::Failed(why) => {
                self.download = None;
                window.push_notification(Notification::error(curly(&why)), cx);
            }
            DictationEvent::Text { .. } => {}
        }
        cx.notify();
    }

    /// Stops the download. The next one resumes it.
    fn cancel_download(&mut self, cx: &mut Context<Self>) {
        self.download = None;
        cx.notify();
    }

    /// What shows beside the button. That's the download's or the model's
    /// step, else the words that may still change.
    pub fn words(&self) -> Option<SharedString> {
        if let Some(download) = &self.download {
            return Some(download.step.label().into());
        }
        let recording = self.recording.as_ref()?;
        match recording.preparing {
            Some(step) => Some(step.label().into()),
            None => (!recording.tentative.is_empty()).then(|| recording.tentative.clone()),
        }
    }
}

impl Render for Dictation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let speech = self.speech.read(cx);
        let status = speech.status();
        let shown = speech.has_recognizer();
        let listening = status.is_capturing()
            && self
                .recording
                .as_ref()
                .is_some_and(|recording| recording.preparing.is_none());
        let words = self.words();
        let muted = cx.theme().muted_foreground;
        h_flex()
            .id("dictation")
            .flex_1()
            .min_w_0()
            .justify_end()
            .gap_2()
            // Esc cancels from the button too, once the keyboard started it.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let escape =
                    event.keystroke.key == "escape" && !event.keystroke.modifiers.modified();
                if escape && this.cancel(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .children(words.map(|words| {
                div()
                    .id("dictation-words")
                    .test_support()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .text_sm()
                    .text_color(muted)
                    .child(words)
            }))
            .when(status.is_capturing(), |this| {
                this.child(
                    div()
                        .id("dictation-waveform")
                        .test_support()
                        .flex_none()
                        .when(listening, |this| {
                            this.role(Role::Status).aria_label("Listening")
                        })
                        .child(SpeechWaveform::new(&self.speech).w(px(48.)).xsmall()),
                )
            })
            .map(|this| match &self.download {
                Some(_) => this.child(
                    Button::new("cancel-download")
                        .ghost()
                        .small()
                        .compact()
                        .icon(IconName::Close)
                        .tooltip("Cancel download")
                        .accessibility_label("Cancel download")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_download(cx))),
                ),
                None if shown => this.child(
                    // The kit's button, whose presses are handled here. A
                    // press starts, a click toggles and a long press holds,
                    // and its own click is left to the keyboard.
                    div()
                        .id("dictate")
                        .test_support()
                        .flex_none()
                        .capture_any_mouse_down(cx.listener(
                            |this, event: &MouseDownEvent, window, cx| {
                                if event.button == MouseButton::Left {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    this.press(window, cx);
                                }
                            },
                        ))
                        .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            if event.button == MouseButton::Left {
                                cx.stop_propagation();
                                this.release(cx);
                            }
                        }))
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.release(cx)),
                        )
                        .child(SpeechButton::new(&self.speech).small()),
                ),
                None => this,
            })
    }
}

/// `text` with a space before it, where it's the recording's first and the
/// box has a word right before the caret.
fn spaced(before: Option<char>, wrote: bool, text: &str) -> String {
    let joins = !wrote
        && !text.starts_with(char::is_whitespace)
        && before.is_some_and(|c| !c.is_whitespace());
    if joins && !text.is_empty() {
        format!(" {text}")
    } else {
        text.to_string()
    }
}

/// The edit that turned `before` into `after`, with the caret at `caret` in
/// `after`. It gives the bytes of `before` it replaced, and the length of
/// what went in their place. What follows the caret was kept, which tells
/// where text typed beside its like went.
fn edit_between(before: &str, after: &str, caret: usize) -> (Range<usize>, usize) {
    let (b, a) = (before.as_bytes(), after.as_bytes());
    let kept = a.len() - caret.min(a.len());
    let mut suffix = b
        .iter()
        .rev()
        .zip(a.iter().rev())
        .take(kept.min(b.len()))
        .take_while(|(x, y)| x == y)
        .count();
    let mut prefix = b
        .iter()
        .zip(a)
        .take(b.len().min(a.len()) - suffix)
        .take_while(|(x, y)| x == y)
        .count();
    while !(before.is_char_boundary(prefix) && after.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    while !(before.is_char_boundary(b.len() - suffix) && after.is_char_boundary(a.len() - suffix)) {
        suffix -= 1;
    }
    (prefix..b.len() - suffix, a.len() - suffix - prefix)
}

/// `ranges` after `replaced` became `inserted` bytes. The part of each before
/// the edit stays, the part after it moves, and the part it replaced goes.
/// Nothing that went in joins a range.
fn follow(ranges: &mut Vec<Range<usize>>, replaced: Range<usize>, inserted: usize) {
    let moved = |offset: usize| offset - replaced.end + replaced.start + inserted;
    let mut followed = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        if range.start < replaced.start {
            followed.push(range.start..range.end.min(replaced.start));
        }
        if range.end > replaced.end {
            followed.push(moved(range.start.max(replaced.end))..moved(range.end));
        }
    }
    followed.retain(|range| !range.is_empty());
    *ranges = followed;
}

#[cfg(test)]
mod tests {
    use super::{edit_between, follow, spaced};

    #[test]
    fn the_first_words_get_a_space_after_a_word() {
        assert_eq!(spaced(Some(':'), false, "Hello"), " Hello");
        assert_eq!(spaced(Some(' '), false, "Hello"), "Hello");
        assert_eq!(spaced(None, false, "Hello"), "Hello");
        assert_eq!(spaced(Some('a'), true, " world"), " world");
        assert_eq!(spaced(Some('a'), true, "world"), "world");
    }

    #[test]
    fn an_edit_is_found_where_the_caret_says() {
        // Typed at the end.
        assert_eq!(edit_between("ab", "abb", 3), (2..2, 1));
        // Typed before its like, where the caret tells which "b" is new.
        assert_eq!(edit_between("ab", "abb", 2), (1..1, 1));
        assert_eq!(edit_between("cat", "ccat", 1), (0..0, 1));
        // Deleted.
        assert_eq!(edit_between("aab", "ab", 1), (1..2, 0));
        // Replaced a selection.
        assert_eq!(edit_between("one two", "one 2", 5), (4..7, 1));
        // Whole characters, also where only their last bytes differ.
        assert_eq!(edit_between("é", "è", 2), (0..2, 2));
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn ranges_follow_edits_and_leave_out_what_went_in() {
        let mut ranges = vec![2..5];
        // Before it, it moves.
        follow(&mut ranges, 0..0, 3);
        assert_eq!(ranges, [5..8]);
        // At its start and its end, it stays as it is.
        follow(&mut ranges, 5..5, 1);
        follow(&mut ranges, 9..9, 1);
        assert_eq!(ranges, [6..9]);
        // Inside it, it splits around what went in.
        follow(&mut ranges, 7..7, 2);
        assert_eq!(ranges, [6..7, 9..11]);
        // Part of it replaced, that part goes.
        follow(&mut ranges, 10..12, 0);
        assert_eq!(ranges, [6..7, 9..10]);
        // All of it, it goes.
        follow(&mut ranges, 0..20, 1);
        assert!(ranges.is_empty());
    }
}
