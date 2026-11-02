//! Dictation, speech typed into the message box and recognized on this
//! computer by Arko's dictation service.
//!
//! The kit's [`SpeechState`] runs each recording, with its [`SpeechButton`]
//! and [`SpeechWaveform`], and captures from the kit's microphone. Its
//! recognizer is Arko's service ([`ArkoRecognizer`]), whose settled words
//! become the kit's committed phrases and whose tentative ones become its
//! hypothesis. The box shows the words at the caret as they come, the
//! tentative ones muted and underlined (the kit's tentative text), and the
//! settled ones go in from there as the recognizer settles them. A recording
//! never sends. While an input method composes, settled words wait in the
//! recording's queue, in order, and the recording isn't over until they're
//! in.
//!
//! A recording owns the words it put in for as long as nothing else changes
//! the box's text, and Esc then takes them out. Once anything else changes
//! the text (typing, a paste, undo), Esc ends the recording and leaves all
//! of it, for undo to take out. Esc and the send key reach a recording before
//! the keymap and the menus do ([`Dictation::intercept`]). The composer takes
//! its text to send, or replaces it whole, only through calls that end or
//! wait for the recording first.
//!
//! The button starts recording on press. A short click keeps it recording
//! until the next click, and a press held past [`HOLD`] records until it's
//! released. The first recording without a model asks before downloading
//! it, and the download runs beside the button without the microphone.
//!
//! Each recording favours the words of the session it types into, such as
//! its identifiers, file names and models, which arko gathers on a thread of
//! its own while the recording starts ([`favour_session_words`]), unless
//! Arko's dictation settings turn session words off.

use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use arko::dictation::{
    CANCEL, DOWNLOAD, DOWNLOAD_TITLE, DictationError, DictationEvent, DictationOptions,
    DictationService, Preparing, Sink, SpeechModel, favour_session_words,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::speech::{
    AudioFormat, AudioInput, RecognitionSession, SpeechButton, SpeechError, SpeechEvent,
    SpeechRecognizer, SpeechSink, SpeechState, SpeechWaveform,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _, h_flex,
    shimmer::ShimmerText,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tokio::sync::mpsc;

use crate::live::{Link, LiveSession};
use crate::transcript::SHIMMER_FPS;
use crate::workers::curly;

/// A press on the button held this long records until it's released.
pub const HOLD: Duration = Duration::from_millis(300);

/// The key context of the dictation's controls, which Esc and the send key
/// reach a recording from, as they do from the composer's.
const CONTEXT: &str = "Dictation";

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
    /// Opens a dictation whose events go to `sink`, favouring the words of
    /// the session `session` links to, if any. A model that isn't
    /// downloaded is [`DictationError::NeedsDownload`], unless
    /// `options.download`.
    fn open(
        &self,
        options: DictationOptions,
        sink: Sink,
        session: Option<&Link>,
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
        session: Option<&Link>,
    ) -> Result<Box<dyn Opened>, DictationError> {
        let service = DictationService::shared(&self.root);
        // A missing model downloads on its own, never inside a recording, so
        // the microphone never runs through a download. Whether that asks
        // first is up to `Dictation::offer_download`.
        let model = service.model();
        if !options.download && !model.is_installed(&self.root) {
            return Err(DictationError::NeedsDownload(model));
        }
        let dictation = service.open(options, sink)?;
        if let Some(link) = session {
            favour_session_words(&dictation, &self.root, &link.feed, &link.details);
        }
        Ok(Box::new(dictation))
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
    DictationOptions { download }
}

/// The kit's recognizer on Arko's dictation service. Each event goes to the
/// kit's state and to the [`Dictation`] whose recording it is.
struct ArkoRecognizer {
    owner: WeakEntity<Dictation>,
    window: AnyWindowHandle,
    /// The [`Dictation`]'s session, which it starts recordings for while it
    /// updates.
    session: Rc<RefCell<Option<WeakEntity<LiveSession>>>>,
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
        let session = self.session.borrow().as_ref().and_then(WeakEntity::upgrade);
        let opened = service
            .open(
                options(false),
                Box::new(move |update| {
                    let _ = sender.send(update.event);
                }),
                session.as_ref().and_then(|session| session.read(cx).link()),
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
    /// The words it put in, while it still owns them.
    owned: Option<Owned>,
    /// Some of its words went in.
    wrote: bool,
    /// The words after them that may still change.
    tentative: SharedString,
    /// The box shows its latest words. They wait while an input method
    /// composes.
    shown: bool,
    /// The character before the caret that the shown words join, which
    /// decides whether a space goes before them.
    after: Option<char>,
    /// Making the model ready, before it listens.
    preparing: Option<Preparing>,
    /// Settled words waiting to go in while an input method composes, in
    /// the order they came. Only the front goes in, so none overtakes
    /// another.
    queue: String,
    /// The kit ended the recording, which is over once its queue is in.
    ended: Option<Ended>,
}

/// How the kit ended a recording.
enum Ended {
    Finished,
    /// It failed, which is said once the queued words are in.
    Failed(SpeechError),
}

/// Where a recording's words are in the box, in order, as its own
/// insertions moved them. They hold while the text is as the last of those
/// insertions left it, at `revision`.
struct Owned {
    ranges: Vec<Range<usize>>,
    revision: u64,
}

/// The model downloading for the recordings to come. Dropping it stops the
/// download, and the next one resumes it.
struct Download {
    _opened: Box<dyn Opened>,
    step: Preparing,
    _events: Task<()>,
}

impl Download {
    /// Bytes are still coming, which is when it can be cancelled.
    fn is_downloading(&self) -> bool {
        matches!(self.step, Preparing::Downloading { .. })
    }
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
    /// The session the box writes to, whose words recordings favour.
    session: Rc<RefCell<Option<WeakEntity<LiveSession>>>>,
    recording: Option<Recording>,
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
        let session = Rc::default();
        let recognizer = ArkoRecognizer {
            owner: cx.weak_entity(),
            window: window.window_handle(),
            session: Rc::clone(&session),
        };
        let input = cx
            .try_global::<Speech>()
            .and_then(|speech| speech.input.clone());
        let speech = cx.new(|cx| {
            let state = SpeechState::new(cx)
                .recognizer(recognizer)
                // Windows' own recognizer sends the audio to Microsoft.
                .system_fallback(false)
                .stop_timeout(LAST_WORDS)
                // Last words that don't come in time end it as a failure,
                // not as if they had come.
                .timeout_is_error(true);
            match input {
                Some(input) => state.input(input),
                None => state,
            }
        });
        let ours = window.window_handle();
        let this = cx.weak_entity();
        let subscriptions = vec![
            cx.subscribe_in(&speech, window, Self::on_speech_event),
            // A composition can end without a change to the text, as Enter
            // ends one, so every redraw of the box may let the queue in.
            cx.observe_in(&textarea, window, |this, _, window, cx| {
                this.drain(window, cx)
            }),
            // The button and the waveform follow the state.
            cx.observe(&speech, |_, _, cx| cx.notify()),
            cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle() == ours {
                    this.update(cx, |this, cx| this.intercept(event, window, cx))
                        .ok();
                }
            }),
        ];
        Self {
            speech,
            textarea,
            session,
            recording: None,
            download: None,
            press: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn speech(&self) -> &Entity<SpeechState> {
        &self.speech
    }

    /// The session the box writes to, whose words recordings favour, or
    /// none (a new session's box).
    pub fn set_session(&mut self, session: Option<WeakEntity<LiveSession>>) {
        *self.session.borrow_mut() = session;
    }

    /// Whether a recording is on, from its start until its last words are in
    /// the box, queued ones included.
    pub fn is_recording(&self, cx: &App) -> bool {
        self.recording.is_some() || self.speech.read(cx).status().is_active()
    }

    /// Starts recording, unless one is on or the model downloads.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.download.is_none() && !self.is_recording(cx) {
            self.speech.update(cx, |speech, cx| speech.start(cx));
        }
    }

    /// Stops recording, and lets the last words come.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.press = None;
        self.speech.update(cx, |speech, cx| speech.stop(cx));
    }

    /// Ends the recording before the box's text is replaced whole (another
    /// session's draft, a message to edit, the input history). The words it
    /// put in stay with the text they were in, and those still to come go
    /// nowhere, queued ones included.
    pub fn end(&mut self, cx: &mut Context<Self>) {
        self.clear_tentative(cx);
        self.recording = None;
        self.press = None;
        self.speech.update(cx, |speech, cx| speech.cancel(cx));
    }

    /// Esc while recording. The recording ends at once, and the words it put
    /// in leave the box while it still owns them. Once something else
    /// changed the text they all stay. Its queued words never go in.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.press = None;
        self.clear_tentative(cx);
        let owned = self.recording.take().and_then(|recording| recording.owned);
        let revision = self.textarea.read(cx).revision();
        if let Some(owned) = owned.filter(|owned| owned.revision == revision) {
            self.textarea.update(cx, |textarea, cx| {
                for range in owned.ranges.into_iter().rev() {
                    textarea.set_selected_range(range, cx);
                    textarea.replace("", window, cx);
                }
            });
        }
        self.speech.update(cx, |speech, cx| speech.cancel(cx));
        cx.notify();
    }

    /// Esc and the send key while a recording is on, from the box or the
    /// button, before the keymap (Vim's included) and the menus see them.
    /// Esc cancels, and the send key stops the recording so its last words
    /// come before anything goes. An input method that composes keeps its
    /// keys.
    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_recording(cx) {
            return;
        }
        let ours = event
            .context_stack
            .iter()
            .any(|context| context.contains(crate::composer::CONTEXT) || context.contains(CONTEXT));
        if !ours || self.is_composing(window, cx) {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.cancel(window, cx);
        } else if keystroke.key == "enter" && self.sends(keystroke.modifiers, cx) {
            self.stop(cx);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    /// Whether Enter with `modifiers` is a chord that sends.
    fn sends(&self, modifiers: Modifiers, cx: &App) -> bool {
        let rest = Modifiers {
            shift: false,
            ..modifiers
        };
        let secondary = rest == Modifiers::secondary_key();
        (secondary || rest == Modifiers::none())
            && self
                .textarea
                .read(cx)
                .is_submit_chord(secondary, modifiers.shift)
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
        if self.is_recording(cx) || !available {
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
                    owned: Some(Owned {
                        ranges: Vec::new(),
                        revision: self.textarea.read(cx).revision(),
                    }),
                    wrote: false,
                    tentative: SharedString::default(),
                    shown: true,
                    after: None,
                    preparing: None,
                    queue: String::new(),
                    ended: None,
                });
            }
            SpeechEvent::Partial(_) => {}
            // Its words are the user's once its queue is in.
            SpeechEvent::Final(_) => {
                self.press = None;
                self.clear_tentative(cx);
                if let Some(recording) = &mut self.recording {
                    recording.ended = Some(Ended::Finished);
                }
                self.drain(window, cx);
            }
            SpeechEvent::Cancelled => {
                self.clear_tentative(cx);
                self.recording = None;
                self.press = None;
            }
            SpeechEvent::Error(error) => {
                self.press = None;
                self.clear_tentative(cx);
                match &mut self.recording {
                    Some(recording) => {
                        recording.ended = Some(Ended::Failed(error.clone()));
                        self.drain(window, cx);
                    }
                    None => self.failed(error, false, window, cx),
                }
            }
        }
        cx.notify();
    }

    /// An event of the recording's dictation, after the kit's state had it.
    fn heard(&mut self, event: &DictationEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        match event {
            DictationEvent::Preparing(step) => recording.preparing = Some(*step),
            DictationEvent::Listening => recording.preparing = None,
            DictationEvent::Text { settled, tentative } => {
                recording.queue.push_str(settled);
                recording.tentative = tentative.clone().into();
                recording.shown = false;
                self.drain(window, cx);
            }
            DictationEvent::Done | DictationEvent::Failed(_) => {}
        }
        cx.notify();
    }

    /// The tentative words go from the box, and the recording's with them.
    fn clear_tentative(&mut self, cx: &mut Context<Self>) {
        if let Some(recording) = &mut self.recording {
            recording.tentative = SharedString::default();
        }
        self.textarea
            .update(cx, |textarea, cx| textarea.clear_tentative_text(cx));
    }

    /// The character before the caret.
    fn before_caret(&self, cx: &App) -> Option<char> {
        let textarea = self.textarea.read(cx);
        textarea.text().chars_at(textarea.cursor()).prev()
    }

    /// Shows the recording's latest words at the caret and puts its queue
    /// in, unless an input method composes, and ends a recording the kit
    /// ended once its queue is in. Words shown again join what's before the
    /// caret now, which a move or a finished composition changed.
    fn drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((queued, shown, after)) = self.recording.as_ref().map(|recording| {
            (
                !recording.queue.is_empty(),
                recording.shown,
                recording.after,
            )
        }) else {
            return;
        };
        let moved = shown && self.before_caret(cx) != after;
        if queued || !shown || moved {
            if self.is_composing(window, cx) {
                return;
            }
            self.put_in(window, cx);
        }
        let ended = self
            .recording
            .as_ref()
            .is_some_and(|recording| recording.ended.is_some());
        if ended {
            let ended = self.recording.take().and_then(|recording| recording.ended);
            if let Some(Ended::Failed(error)) = ended {
                self.failed(&error, true, window, cx);
            }
            cx.notify();
        }
    }

    /// Shows the recording's queued and tentative words at the caret,
    /// wherever it is, as the box's tentative text, and puts the queued ones
    /// in from there. It keeps hold of what went in while nothing else
    /// changed the text.
    fn put_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((wrote, queue, tentative)) = self.recording.as_ref().map(|recording| {
            (
                recording.wrote,
                recording.queue.clone(),
                recording.tentative.clone(),
            )
        }) else {
            return;
        };
        let before = self.textarea.read(cx).revision();
        let (at, len, landed, after) = self.textarea.update(cx, |textarea, cx| {
            let at = textarea.cursor();
            let text = textarea.text().to_string();
            let previous = text[..at].chars().next_back();
            let settled = spaced(previous, wrote, &queue);
            let last = settled.chars().next_back().or(previous);
            let tentative = spaced(last, wrote || !settled.is_empty(), &tentative);
            textarea.set_tentative_text(format!("{settled}{tentative}"), cx);
            if settled.is_empty() || !textarea.commit_tentative_text(settled.len(), window, cx) {
                return (at, 0, false, textarea.revision());
            }
            // The words, in one piece where the caret was.
            let landed = textarea.cursor() == at + settled.len()
                && textarea.text().to_string().get(at..at + settled.len())
                    == Some(settled.as_str());
            (at, settled.len(), landed, textarea.revision())
        });
        let joins = self.before_caret(cx);
        let Some(recording) = &mut self.recording else {
            return;
        };
        recording.shown = true;
        recording.after = joins;
        if len == 0 {
            return;
        }
        recording.queue.clear();
        recording.wrote = true;
        recording.owned = recording
            .owned
            .take()
            .filter(|owned| owned.revision == before && landed)
            .map(|mut owned| {
                own(&mut owned.ranges, at, len);
                owned.revision = after;
                owned
            });
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
        if error.is_timeout() {
            window.push_notification(Notification::warning(TOO_SLOW), cx);
            return;
        }
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
        let opened = match service.open(options(true), sink, None) {
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

    /// Stops the download, while bytes are coming. The next one resumes it.
    /// Once they are in, the model is prepared and a click that was already
    /// on its way does nothing.
    pub(crate) fn cancel_download(&mut self, cx: &mut Context<Self>) {
        if self.download.as_ref().is_some_and(Download::is_downloading) {
            self.download = None;
            cx.notify();
        }
    }

    /// The download's step or the model's, beside the button.
    fn step(&self) -> Option<Preparing> {
        if let Some(download) = &self.download {
            return Some(download.step);
        }
        self.recording.as_ref()?.preparing
    }

    /// What shows beside the button, the download's or the model's step.
    /// The words heard show in the box.
    pub fn words(&self) -> Option<SharedString> {
        self.step().map(|step| step.label().into())
    }
}

impl Render for Dictation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let speech = self.speech.read(cx);
        let status = speech.status();
        let shown = speech.has_recognizer();
        // A new recording waits for the last one's queued words to go in.
        let waits = self.recording.is_some() && !status.is_active();
        let listening = status.is_capturing()
            && self
                .recording
                .as_ref()
                .is_some_and(|recording| recording.preparing.is_none());
        let step = self.step();
        let muted = cx.theme().muted_foreground;
        h_flex()
            .id("dictation")
            .key_context(CONTEXT)
            .flex_1()
            .min_w_0()
            .justify_end()
            .gap_2()
            .children(step.map(|step| {
                // Downloading shows its count. The steps after it have none,
                // so the label shimmers (still, with reduced motion).
                let words: AnyElement =
                    if !matches!(step, Preparing::Downloading { .. }) && !cx.reduce_motion() {
                        div()
                            .id("dictation-shimmer")
                            .test_support()
                            .child(
                                ShimmerText::new(step.label())
                                    .id("dictation-shimmer-text")
                                    .max_fps(SHIMMER_FPS),
                            )
                            .into_any_element()
                    } else {
                        SharedString::from(step.label()).into_any_element()
                    };
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
                Some(download) if download.is_downloading() => this.child(
                    Button::new("cancel-download")
                        .ghost()
                        .small()
                        .compact()
                        .icon(IconName::Close)
                        .tooltip("Cancel download")
                        .accessibility_label("Cancel download")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_download(cx))),
                ),
                Some(_) => this,
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
                        .child(SpeechButton::new(&self.speech).small().disabled(waits)),
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

/// `ranges` after the recording put `len` bytes in at `at`. The ranges after
/// that point move, one it went into grows, and the new words join any
/// range they touch.
fn own(ranges: &mut Vec<Range<usize>>, at: usize, len: usize) {
    for range in ranges.iter_mut() {
        if range.start >= at {
            range.start += len;
            range.end += len;
        } else if range.end > at {
            range.end += len;
        }
    }
    ranges.push(at..at + len);
    ranges.sort_by_key(|range| range.start);
    let mut joined: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match joined.last_mut() {
            Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    *ranges = joined;
}

#[cfg(test)]
mod tests {
    use super::{own, spaced};

    #[test]
    fn the_first_words_get_a_space_after_a_word() {
        assert_eq!(spaced(Some(':'), false, "Hello"), " Hello");
        assert_eq!(spaced(Some(' '), false, "Hello"), "Hello");
        assert_eq!(spaced(None, false, "Hello"), "Hello");
        assert_eq!(spaced(Some('a'), true, " world"), " world");
        assert_eq!(spaced(Some('a'), true, "world"), "world");
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn the_recordings_own_words_move_its_ranges() {
        let mut ranges = Vec::new();
        own(&mut ranges, 5, 3);
        assert_eq!(ranges, [5..8]);
        // Before them, they move, and the two stay apart.
        own(&mut ranges, 0, 4);
        assert_eq!(ranges, [0..4, 9..12]);
        // Right after one, it grows.
        own(&mut ranges, 12, 2);
        assert_eq!(ranges, [0..4, 9..14]);
        // Inside one, it grows.
        own(&mut ranges, 10, 1);
        assert_eq!(ranges, [0..4, 9..15]);
        // Right after one, it grows, and the one after moves.
        own(&mut ranges, 4, 5);
        assert_eq!(ranges, [0..9, 14..20]);
    }
}
