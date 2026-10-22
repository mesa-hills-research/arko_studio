//! A window of spinner styles side by side, on a light and a dark panel and at the sizes the app
//! uses, to choose from and to see whether this machine draws them evenly.
//!
//! ```sh
//! cargo run --release --example spinner_lab
//! ```
//!
//! Every style turns, fades or pulses as it paints, with GPUI's `with_paint_animation`, so the
//! window never builds a view again for a step. A style that fades is made of several sprites or
//! quads whose opacity runs on the same clock, because GPUI's paint effects are an opacity and a
//! rotation of sprites. The rate buttons set how often the styles draw, "Display" being a frame
//! for each frame the display shows. Pause holds every style still, as Reduce Motion does.
//!
//! The readout counts every frame the window draws, so frames that moving the pointer over the
//! window asks for count too. Keep the pointer still to read it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use arko_studio::{assets::Assets, fonts};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, FontOverrides, Icon, IconName, Selectable as _,
    Sizable as _, StyledExt as _, Theme, ThemeMode, button::Button, h_flex, v_flex,
};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, Context,
    Entity, Hsla, IntoElement, PaintEffect, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, canvas,
    div, ease_in_out, percentage, prelude::FluentBuilder as _, px, rgb, size,
};

/// The sizes of the icons in the app, in logical pixels, and a large one to judge a shape by.
const SIZES: [f32; 4] = [12., 16., 24., 48.];

/// How often the styles draw.
#[derive(Clone, Copy, PartialEq)]
enum Rate {
    Fps(u32),
    /// A frame for each frame the display shows.
    Display,
}

impl Rate {
    const ALL: [Rate; 5] = [
        Rate::Fps(12),
        Rate::Fps(24),
        Rate::Fps(30),
        Rate::Fps(60),
        Rate::Display,
    ];

    fn label(self) -> SharedString {
        match self {
            Rate::Fps(fps) => format!("{fps}").into(),
            Rate::Display => "Display".into(),
        }
    }
}

/// How fast the styles go, as a multiple of the speed each is drawn at.
const SPEEDS: [f32; 6] = [0.25, 0.5, 1., 1.5, 2., 3.];
const NORMAL_SPEED: usize = 2;

/// What a style needs to draw one of its cells.
struct Cell<'a> {
    panel: &'static str,
    size: f32,
    color: Hsla,
    rate: Rate,
    speed: f32,
    svgs: &'a Svgs,
}

impl Cell<'_> {
    /// An animation of `period_ms` at the speed and rate chosen. Its phase follows the app's shared
    /// clock, so a cell's parts, and the cells beside it, keep in step.
    fn animation(&self, period_ms: f32) -> Animation {
        self.animation_at(period_ms, self.rate)
    }

    fn animation_at(&self, period_ms: f32, rate: Rate) -> Animation {
        let animation =
            Animation::new(Duration::from_secs_f32(period_ms / 1000. / self.speed)).repeat_synced();
        match rate {
            Rate::Fps(fps) => animation.with_max_fps(fps as f32),
            Rate::Display => animation,
        }
    }

    fn id(&self, style: char, part: usize) -> SharedString {
        format!("{}-{style}-{}-{part}", self.panel, self.size).into()
    }

    fn square(&self) -> gpui_kit::Div {
        div().relative().flex_none().size(px(self.size))
    }

    /// A sprite from SVG data that covers the cell.
    fn sprite(&self, data: &[u8]) -> Icon {
        Icon::empty()
            .data(data)
            .with_size(px(self.size))
            .text_color(self.color)
    }

    /// The kit's icon `name`, in the cell.
    fn sprite_icon(&self, name: IconName) -> Icon {
        Icon::new(name)
            .with_size(px(self.size))
            .text_color(self.color)
    }

    /// A sprite from SVG data that covers the cell and lies over the others in it.
    fn layer(&self, data: &[u8]) -> Icon {
        self.sprite(data).absolute().top_0().left_0()
    }
}

/// A style of spinner.
struct Style {
    letter: char,
    name: &'static str,
    /// What it draws with, for the cost.
    cost: &'static str,
    build: fn(&Cell) -> AnyElement,
}

const STYLES: [Style; 9] = [
    Style {
        letter: 'A',
        name: "Before, eased at 30 fps",
        cost: "1 sprite turned",
        build: before,
    },
    Style {
        letter: 'B',
        name: "Spokes at a steady speed",
        cost: "1 sprite turned",
        build: spokes_turning,
    },
    Style {
        letter: 'C',
        name: "Arc that grows and shrinks",
        cost: "24 sprites faded",
        build: growing_arc,
    },
    Style {
        letter: 'D',
        name: "Three dots in turn",
        cost: "3 quads faded",
        build: three_dots,
    },
    Style {
        letter: 'E',
        name: "Spokes that fade around",
        cost: "8 sprites faded",
        build: fading_spokes,
    },
    Style {
        letter: 'F',
        name: "Ring with a tail",
        cost: "1 sprite turned",
        build: ring_with_a_tail,
    },
    Style {
        letter: 'G',
        name: "Breathing dot",
        cost: "1 quad faded",
        build: breathing_dot,
    },
    Style {
        letter: 'H',
        name: "Arko mark",
        cost: "4 sprites faded",
        build: arko_mark,
    },
    Style {
        letter: 'I',
        name: "Comet ring",
        cost: "12 sprites faded",
        build: comet_ring,
    },
];

/// A fade for `element`, `value` of the animation's phase.
fn fade(
    element: impl IntoElement + 'static,
    id: SharedString,
    animation: Animation,
    opacity: impl Fn(f32) -> f32 + 'static,
) -> AnyElement {
    element
        .with_paint_animation(id, animation, move |phase| {
            PaintEffect::opacity(opacity(phase))
        })
        .into_any_element()
}

/// How lit part `k` of `n` is when the head of a chase is at `phase` of its way round, a turn
/// being 1. The head lights a part, and the part fades to `floor` as the head leaves it.
fn chase(phase: f32, k: usize, n: usize, floor: f32, attack: f32) -> f32 {
    let behind = (phase - k as f32 / n as f32).rem_euclid(1.);
    if behind < attack {
        floor + (1. - floor) * (behind / attack)
    } else {
        let left = (behind - attack) / (1. - attack);
        floor + (1. - floor) * (1. - left).powf(1.6)
    }
}

// ----- A and B: the eight spokes of the kit's spinner icon -----

/// The spinner of the kit before it turned evenly, whatever the rate chosen: the spokes, eased
/// in and out, 30 frames a second, 0.8 s a turn.
fn before(cell: &Cell) -> AnyElement {
    let animation = cell
        .animation_at(800., Rate::Fps(30))
        .with_easing(ease_in_out);
    cell.sprite_icon(IconName::Loader)
        .with_paint_animation(cell.id('A', 0), animation, |delta| {
            PaintEffect::rotation(percentage(delta).into())
        })
        .into_any_element()
}

fn spokes_turning(cell: &Cell) -> AnyElement {
    cell.sprite_icon(IconName::Loader)
        .with_paint_animation(cell.id('B', 0), cell.animation(800.), |delta| {
            PaintEffect::rotation(percentage(delta).into())
        })
        .into_any_element()
}

// ----- C: an arc that grows and shrinks while it turns -----

/// How much of the circle from `a` for `a_len` degrees, and from `b` for `b_len`, overlap.
fn overlap(a: f32, a_len: f32, b: f32, b_len: f32) -> f32 {
    [-360., 0., 360.]
        .into_iter()
        .map(|shift| {
            let low = a.max(b + shift);
            let high = (a + a_len).min(b + shift + b_len);
            (high - low).max(0.)
        })
        .sum()
}

fn growing_arc(cell: &Cell) -> AnyElement {
    let segments = cell.svgs.arc.len();
    let width = 360. / segments as f32;
    let mut square = cell.square();
    for (k, data) in cell.svgs.arc.iter().enumerate() {
        let from = k as f32 * width;
        square = square.child(fade(
            cell.layer(data),
            cell.id('C', k),
            cell.animation(1400.),
            move |phase| {
                // The tail goes round twice in a cycle. The head leaves it and falls back.
                let tail = (720. * phase).rem_euclid(360.);
                let length = 30. + 230. * (PI * phase).sin().powi(2);
                (overlap(tail, length, from, width) / width).clamp(0., 1.)
            },
        ));
    }
    square.into_any_element()
}

// ----- D, G: dots, drawn as quads -----

fn dot(diameter: f32, color: Hsla) -> gpui_kit::Div {
    div().size(px(diameter)).rounded_full().bg(color)
}

fn three_dots(cell: &Cell) -> AnyElement {
    let diameter = (cell.size * 0.26).max(2.).round();
    let gap = (cell.size - 3. * diameter) / 2.;
    let mut row = h_flex()
        .size(px(cell.size))
        .flex_none()
        .items_center()
        .gap(px(gap));
    for k in 0..3 {
        row = row.child(fade(
            dot(diameter, cell.color),
            cell.id('D', k),
            cell.animation(1000.),
            move |phase| {
                // Each dot swells and fades once a cycle, a third of a cycle after the one before.
                let bell = 0.5 - 0.5 * (TAU * (phase - k as f32 / 3.)).cos();
                0.2 + 0.8 * bell * bell
            },
        ));
    }
    row.into_any_element()
}

fn breathing_dot(cell: &Cell) -> AnyElement {
    let diameter = (cell.size * 0.5).round().max(3.);
    h_flex()
        .size(px(cell.size))
        .flex_none()
        .items_center()
        .justify_center()
        .child(fade(
            dot(diameter, cell.color),
            cell.id('G', 0),
            cell.animation(1200.),
            |phase| 0.2 + 0.8 * (0.5 - 0.5 * (TAU * phase).cos()),
        ))
        .into_any_element()
}

// ----- E: the classic activity indicator -----

fn fading_spokes(cell: &Cell) -> AnyElement {
    let mut square = cell.square();
    for (k, data) in cell.svgs.spokes.iter().enumerate() {
        square = square.child(fade(
            cell.layer(data),
            cell.id('E', k),
            cell.animation(1000.),
            move |phase| chase(phase, k, 8, 0.14, 0.04),
        ));
    }
    square.into_any_element()
}

// ----- F: a ring with a bright head and a fading tail, turned -----

fn ring_with_a_tail(cell: &Cell) -> AnyElement {
    cell.sprite(&cell.svgs.tail)
        .with_paint_animation(cell.id('F', 0), cell.animation(900.), |delta| {
            PaintEffect::rotation(percentage(delta).into())
        })
        .into_any_element()
}

// ----- H: the Arko mark, its nodes lighting in turn -----

fn arko_mark(cell: &Cell) -> AnyElement {
    let orange: Hsla = rgb(0xD5642A).into();
    let layer = |data: &[u8]| {
        Icon::empty()
            .data(data)
            .with_size(px(cell.size))
            .text_color(orange)
            .absolute()
            .top_0()
            .left_0()
    };
    let mut square = cell.square().child(layer(&cell.svgs.mark));
    for (k, data) in cell.svgs.nodes.iter().enumerate() {
        square = square.child(fade(
            layer(data),
            cell.id('H', k),
            cell.animation(1500.),
            move |phase| chase(phase, k, 3, 0., 0.08),
        ));
    }
    square.into_any_element()
}

// ----- I: a comet, a ring of segments with a bright head -----

fn comet_ring(cell: &Cell) -> AnyElement {
    let segments = cell.svgs.comet.len();
    let mut square = cell.square();
    for (k, data) in cell.svgs.comet.iter().enumerate() {
        square = square.child(fade(
            cell.layer(data),
            cell.id('I', k),
            cell.animation(900.),
            move |phase| chase(phase, k, segments, 0.1, 0.03),
        ));
    }
    square.into_any_element()
}

// ----- the SVGs the styles draw with -----

/// The sprites of the styles, as SVG data.
struct Svgs {
    /// The eight spokes of the kit's loader icon, one each, clockwise from the top.
    spokes: Vec<Vec<u8>>,
    /// The segments of a ring, clockwise from the top.
    arc: Vec<Vec<u8>>,
    comet: Vec<Vec<u8>>,
    /// A ring whose strokes fade from nothing to solid, clockwise.
    tail: Vec<u8>,
    /// The Arko mark for small sizes, dimmed, and its three nodes alone.
    mark: Vec<u8>,
    nodes: Vec<Vec<u8>>,
}

fn svg(body: &str, view_box: u32) -> Vec<u8> {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view_box} {view_box}">{body}</svg>"#
    )
    .into_bytes()
}

/// The arc of the ring from `from` to `to` degrees clockwise from the top, as a stroke.
fn ring_arc(from: f32, to: f32, opacity: f32) -> String {
    let (radius, centre) = (9.0_f32, 12.0_f32);
    let point = |degrees: f32| {
        let angle = degrees.to_radians();
        (centre + radius * angle.sin(), centre - radius * angle.cos())
    };
    let ((x0, y0), (x1, y1)) = (point(from), point(to));
    let large = u8::from(to - from > 180.);
    format!(
        r##"<path d="M{x0:.3} {y0:.3}A{radius} {radius} 0 {large} 1 {x1:.3} {y1:.3}" fill="none" stroke="#000" stroke-width="2.6" stroke-opacity="{opacity:.3}"/>"##
    )
}

impl Svgs {
    fn new() -> Self {
        // The paths of the kit's loader icon, clockwise from the top.
        let loader = [
            "M12 2v4",
            "m16.2 7.8 2.9-2.9",
            "M18 12h4",
            "m16.2 16.2 2.9 2.9",
            "M12 18v4",
            "m4.9 19.1 2.9-2.9",
            "M2 12h4",
            "m4.9 4.9 2.9 2.9",
        ];
        let spokes = loader
            .iter()
            .map(|d| {
                svg(
                    &format!(
                        r##"<path d="{d}" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round"/>"##
                    ),
                    24,
                )
            })
            .collect();
        // Segments overlap by a little, so no seam shows between two.
        let segments = |n: usize| -> Vec<Vec<u8>> {
            let width = 360. / n as f32;
            (0..n)
                .map(|k| {
                    let from = k as f32 * width;
                    svg(&ring_arc(from - 0.35, from + width + 0.35, 1.), 24)
                })
                .collect()
        };
        let tail_body: String = (0..60)
            .map(|k| {
                let from = k as f32 * 5.;
                let opacity = (k as f32 / 59.).powf(1.4);
                ring_arc(from, from + 5.6, opacity)
            })
            .collect();
        // The mark for 16 to 32 pixels, on its 1024 grid. The nodes are the circles the mark
        // draws at the ends of its links.
        let mark = svg(
            r##"<g fill="#000" opacity=".3"><path fill-rule="evenodd" d="M766.5 384A352 352 0 1 0 66.5 458.1A96 96 0 0 0 231.9 543.6A224 224 0 1 1 637.7 384ZM128 480a32 32 0 1 0 64 0a32 32 0 1 0 -64 0Z"/><path d="M640 576L640 608A224 224 0 0 1 377.2 828.6A96 96 0 1 0 331.3 949.7A352 352 0 0 0 640 879.5L640 960L768 960L768 576Z"/><circle cx="480" cy="480" r="96"/><rect x="480" y="448" width="368" height="64"/><path d="M832 304L1008 480L832 656Z"/><path fill="none" stroke="#000" stroke-width="64" d="M224 480H480M180.2 540.7L288 864M480 480L288 864"/></g>"##,
            1024,
        );
        let nodes = [
            r##"<path fill="#000" fill-rule="evenodd" d="M64 480a96 96 0 1 0 192 0a96 96 0 1 0 -192 0ZM128 480a32 32 0 1 0 64 0a32 32 0 1 0 -64 0Z"/>"##,
            r##"<circle cx="480" cy="480" r="96" fill="#000"/>"##,
            r##"<circle cx="288" cy="864" r="96" fill="#000"/>"##,
        ]
        .iter()
        .map(|body| svg(body, 1024))
        .collect();
        Self {
            spokes,
            arc: segments(24),
            comet: segments(12),
            tail: svg(&tail_body, 24),
            mark,
            nodes,
        }
    }
}

// ----- what the window measures -----

/// When the window drew its last frames.
struct Frames {
    drawn: VecDeque<Instant>,
}

/// How the frames of the last seconds went.
struct Pace {
    fps: f32,
    slowest_ms: f32,
    late: usize,
}

impl Frames {
    fn record(&mut self) {
        let now = Instant::now();
        self.drawn.push_back(now);
        while self
            .drawn
            .front()
            .is_some_and(|first| now.duration_since(*first) > Duration::from_secs(3))
        {
            self.drawn.pop_front();
        }
    }

    /// None while nothing is drawn.
    fn pace(&self) -> Option<Pace> {
        let last = *self.drawn.back()?;
        if last.elapsed() > Duration::from_millis(600) || self.drawn.len() < 3 {
            return None;
        }
        let mut gaps: Vec<f32> = self
            .drawn
            .iter()
            .zip(self.drawn.iter().skip(1))
            .map(|(a, b)| b.duration_since(*a).as_secs_f32() * 1000.)
            .collect();
        let in_the_last_second = self
            .drawn
            .iter()
            .filter(|at| last.duration_since(**at) <= Duration::from_secs(1))
            .count();
        let first_of_that_second = self
            .drawn
            .iter()
            .find(|at| last.duration_since(**at) <= Duration::from_secs(1))?;
        let span = last.duration_since(*first_of_that_second).as_secs_f32();
        let slowest_ms = gaps.iter().copied().fold(0., f32::max);
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = gaps[gaps.len() / 2];
        Some(Pace {
            fps: if span > 0. {
                (in_the_last_second - 1) as f32 / span
            } else {
                0.
            },
            slowest_ms,
            late: gaps.iter().filter(|gap| **gap > median * 1.5).count(),
        })
    }
}

/// The numbers in the corner of the window.
struct Readout {
    frames: Rc<RefCell<Frames>>,
}

impl Render for Readout {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let text = match self.frames.borrow().pace() {
            Some(pace) => format!(
                "{:.1} frames a second, slowest {:.1} ms, {} late",
                pace.fps, pace.slowest_ms, pace.late
            ),
            None => "No frames drawn".to_string(),
        };
        div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(text)
    }
}

// ----- the panels -----

struct Look {
    name: &'static str,
    background: u32,
    foreground: u32,
}

const LIGHT: Look = Look {
    name: "light",
    background: 0xF7F7F5,
    foreground: 0x5C616A,
};
const DARK: Look = Look {
    name: "dark",
    background: 0x1D1F23,
    foreground: 0xA6ABB4,
};

/// The grid of styles and sizes on both panels.
struct Grid {
    svgs: Svgs,
    rate: Rate,
    speed: f32,
}

impl Grid {
    fn panel(&self, look: &Look, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let color: Hsla = rgb(look.foreground).into();
        v_flex()
            .flex_1()
            .min_w_0()
            .p_3()
            .gap_1()
            .rounded_md()
            .bg(Hsla::from(rgb(look.background)))
            .text_color(color)
            .child(
                h_flex()
                    .h_6()
                    .items_center()
                    .child(div().w(px(220.)).text_xs().child(look.name))
                    .children(SIZES.map(|size| {
                        div()
                            .w(px(70.))
                            .flex_none()
                            .text_center()
                            .text_xs()
                            .child(format!("{size} px"))
                    })),
            )
            .children(STYLES.iter().map(|style| {
                h_flex()
                    .h(px(56.))
                    .items_center()
                    .child(
                        v_flex()
                            .w(px(220.))
                            .pr_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_semibold()
                                            .child(style.letter.to_string()),
                                    )
                                    .child(div().text_sm().child(style.name)),
                            )
                            .child(div().text_xs().text_color(muted).child(style.cost)),
                    )
                    .children(SIZES.map(|size| {
                        let cell = Cell {
                            panel: look.name,
                            size,
                            color,
                            rate: self.rate,
                            speed: self.speed,
                            svgs: &self.svgs,
                        };
                        div()
                            .w(px(70.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child((style.build)(&cell))
                    }))
            }))
            .into_any_element()
    }
}

impl Render for Grid {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .gap_3()
            .items_start()
            .child(self.panel(&LIGHT, cx))
            .child(self.panel(&DARK, cx))
    }
}

// ----- the window -----

pub struct Lab {
    grid: Entity<Grid>,
    readout: Entity<Readout>,
    frames: Rc<RefCell<Frames>>,
    rate: usize,
    speed: usize,
    paused: bool,
    /// Whether the system asked for reduced motion when the window opened.
    system_reduced: bool,
}

impl Lab {
    fn new(cx: &mut Context<Self>) -> Self {
        let frames = Rc::new(RefCell::new(Frames {
            drawn: VecDeque::new(),
        }));
        let rate = Rate::ALL.len() - 1;
        let grid = cx.new(|_| Grid {
            svgs: Svgs::new(),
            rate: Rate::ALL[rate],
            speed: SPEEDS[NORMAL_SPEED],
        });
        let readout = cx.new(|_| Readout {
            frames: frames.clone(),
        });
        // The numbers change four times a second, and nothing else draws for them.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let live = this.update(cx, |this, cx| {
                    this.readout.update(cx, |_, cx| cx.notify());
                });
                if live.is_err() {
                    break;
                }
            }
        })
        .detach();
        let system_reduced = cx.reduce_motion();
        Self {
            grid,
            readout,
            frames,
            rate,
            speed: NORMAL_SPEED,
            paused: system_reduced,
            system_reduced,
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let (rate, speed) = (Rate::ALL[self.rate], SPEEDS[self.speed]);
        self.grid.update(cx, |grid, cx| {
            grid.rate = rate;
            grid.speed = speed;
            cx.notify();
        });
        cx.set_reduce_motion(self.paused);
        cx.notify();
    }
}

impl Render for Lab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let frames = self.frames.clone();
        let rates = Rate::ALL.into_iter().enumerate().map(|(ix, rate)| {
            Button::new(("rate", ix))
                .small()
                .outline()
                .label(rate.label())
                .selected(ix == self.rate)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rate = ix;
                    this.apply(cx);
                }))
        });
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .p_4()
            .gap_3()
            // Counts the frames the window draws, as it paints in each of them.
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, _, _| frames.borrow_mut().record(),
                )
                .absolute()
                .size_0(),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_4()
                    .flex_wrap()
                    .child(div().text_lg().font_semibold().child("Spinner lab"))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child("Frames a second"))
                            .children(rates),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_sm().mr_1().child("Speed"))
                            .child(
                                Button::new("slower")
                                    .small()
                                    .outline()
                                    .label("−")
                                    .disabled(self.speed == 0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.speed = this.speed.saturating_sub(1);
                                        this.apply(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .w(px(44.))
                                    .text_center()
                                    .text_sm()
                                    .child(format!("{}×", SPEEDS[self.speed])),
                            )
                            .child(
                                Button::new("faster")
                                    .small()
                                    .outline()
                                    .label("+")
                                    .disabled(self.speed + 1 == SPEEDS.len())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.speed = (this.speed + 1).min(SPEEDS.len() - 1);
                                        this.apply(cx);
                                    })),
                            ),
                    )
                    .child(
                        Button::new("pause")
                            .small()
                            .outline()
                            .label(if self.paused { "Play" } else { "Pause" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.paused = !this.paused;
                                this.apply(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        self.readout
                            .clone()
                            .cached(StyleRefinement::default().w(px(430.)).h(px(20.))),
                    ),
            )
            .when(self.system_reduced && self.paused, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("The system asks for reduced motion, so the styles hold still. Press Play to turn them anyway."),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.grid.clone().cached(StyleRefinement::default().size_full())),
            )
    }
}

/// Starts GPUI Kit, the fonts and the light theme, as the app does.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    for font in fonts::bundled() {
        if let Err(error) = cx.text_system().add_fonts(vec![font]) {
            eprintln!("couldn't load a font ({error})");
        }
    }
    Theme::change(ThemeMode::Light, None, cx);
    Theme::update(cx, |theme| {
        theme.set_font_overrides(
            FontOverrides::new()
                .with_font_family(gpui_kit::assets::fonts::WORK_SANS_FAMILY)
                .with_font_size(px(14.)),
        )
    });
}

/// Opens the lab with `options`.
pub fn open(options: WindowOptions, cx: &mut App) -> Result<(AnyWindowHandle, Entity<Lab>)> {
    gpui_kit::open_window(options, cx, |_, cx| cx.new(Lab::new))
}

/// The lab's window, centred.
pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Spinner lab".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(1240.), px(720.)), cx)),
        ..Default::default()
    }
}

fn main() {
    gpui_kit::application().with_assets(Assets).run(|cx| {
        init(cx);
        if let Err(error) = open(window_options(cx), cx) {
            panic!("couldn’t open the window ({error})");
        }
        cx.activate(true);
    });
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::time::Duration;

    use gpui_screenshot::ScreenshotApp;

    use super::*;

    /// The lab drawn headlessly by mhr-gpui-screenshot, so a change that breaks it shows here.
    /// With `SPINNER_LAB_SHEETS` set to a folder, the frames are also written there as PNG files.
    #[test]
    fn the_lab_draws_styles_that_move() {
        let mut app = ScreenshotApp::with_assets(Assets).unwrap();
        app.update(|cx| {
            init(cx);
            cx.set_reduce_motion(false);
        });
        let window = app
            .open_window_with(size(px(1240.), px(720.)), 1., |options, cx| {
                open(options, cx).map(|(window, _)| window)
            })
            .unwrap();
        let mut shots = Vec::new();
        // 110 ms apart, which none of the styles repeats in.
        for _ in 0..8 {
            shots.push(app.capture(window).unwrap().image);
            app.advance_clock(Duration::from_millis(110));
        }
        if let Some(folder) = std::env::var_os("SPINNER_LAB_SHEETS") {
            std::fs::create_dir_all(&folder).unwrap();
            for (ix, shot) in shots.iter().enumerate() {
                shot.save(std::path::Path::new(&folder).join(format!("lab-{ix}.png")))
                    .unwrap();
            }
        }
        assert!(
            shots[0].pixels().any(|pixel| pixel.0[..3] != [0, 0, 0]),
            "the window is black"
        );
        for pair in shots.windows(2) {
            let changed = pair[0]
                .pixels()
                .zip(pair[1].pixels())
                .filter(|(a, b)| a != b);
            assert!(
                changed.count() > 200,
                "the styles don't move between frames"
            );
        }
    }

    /// Pause holds every style still, and Play lets them go on.
    #[test]
    fn pausing_holds_the_styles_still() {
        let mut app = ScreenshotApp::with_assets(Assets).unwrap();
        app.update(|cx| {
            init(cx);
            cx.set_reduce_motion(false);
        });
        let lab = Rc::new(RefCell::new(None));
        let window = app
            .open_window_with(size(px(1240.), px(720.)), 1., |options, cx| {
                let (window, view) = open(options, cx)?;
                *lab.borrow_mut() = Some(view);
                Ok(window)
            })
            .unwrap();
        let lab = lab.borrow_mut().take().unwrap();
        let set_paused = |app: &mut ScreenshotApp, paused: bool| {
            app.update_window(window, |_, _, cx| {
                lab.update(cx, |lab, cx| {
                    lab.paused = paused;
                    lab.apply(cx);
                })
            })
            .unwrap();
        };
        // The grid, under the controls, whose numbers change as frames are drawn.
        let grid = |app: &mut ScreenshotApp| {
            let shot = app.capture(window).unwrap().image;
            shot.enumerate_pixels()
                .filter(|(_, y, _)| *y > 70)
                .map(|(_, _, pixel)| pixel.0)
                .collect::<Vec<_>>()
        };
        set_paused(&mut app, true);
        let first = grid(&mut app);
        app.advance_clock(Duration::from_millis(130));
        assert!(first == grid(&mut app), "a paused style moved");
        set_paused(&mut app, false);
        app.advance_clock(Duration::from_millis(130));
        let moved = grid(&mut app);
        assert!(first != moved, "the styles stay still after Play");
    }

    /// A window of `copies` cells of one style, in a view the window keeps.
    struct Copies {
        style: usize,
        copies: usize,
        svgs: Svgs,
    }

    impl Render for Copies {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let color: Hsla = rgb(LIGHT.foreground).into();
            let panels = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
            div()
                .flex()
                .flex_wrap()
                .children((0..self.copies).map(|copy| {
                    let cell = Cell {
                        panel: panels[copy % panels.len()],
                        size: 16. + (copy / panels.len()) as f32 * 0.01,
                        color,
                        rate: Rate::Display,
                        speed: 1.,
                        svgs: &self.svgs,
                    };
                    div().size(px(24.)).child((STYLES[self.style].build)(&cell))
                }))
        }
    }

    /// What a window costs in a frame for each cell of each style, in microseconds of processor
    /// time. The frames come from the display, the cells are in a view the window keeps, and the
    /// window draws without rendering, so the cost is the app's, not the graphics driver's.
    ///
    /// ```sh
    /// cargo test --release --example spinner_lab what_a_frame_costs -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn what_a_frame_costs() {
        fn cpu_time() -> Duration {
            std::fs::read_to_string("/proc/thread-self/schedstat")
                .ok()
                .and_then(|stat| stat.split_whitespace().next()?.parse::<u64>().ok())
                .map(Duration::from_nanos)
                .unwrap_or_default()
        }
        const COPIES: usize = 100;
        const FRAMES: usize = 300;
        let mut app = ScreenshotApp::with_assets(Assets).unwrap();
        app.update(|cx| {
            init(cx);
            cx.set_reduce_motion(false);
        });
        let mut baseline = Duration::ZERO;
        for style in std::iter::once(None).chain((0..STYLES.len()).map(Some)) {
            let copies = if style.is_some() { COPIES } else { 1 };
            let window = app
                .open_window_with(size(px(800.), px(800.)), 1., |options, cx| {
                    gpui_kit::open_window(options, cx, |_, cx| {
                        let view = cx.new(|_| Copies {
                            style: style.unwrap_or(3),
                            copies,
                            svgs: Svgs::new(),
                        });
                        cx.new(|_| Kept(view))
                    })
                    .map(|(window, _)| window)
                })
                .unwrap();
            app.capture(window).unwrap();
            let mut best = Duration::MAX;
            for _ in 0..8 {
                let start = cpu_time();
                for _ in 0..FRAMES / 8 {
                    app.advance_clock(Duration::from_micros(16_667));
                    app.update_window(window, |_, window, cx| {
                        window.simulate_next_frame(cx);
                        window.draw(cx).clear(cx);
                    })
                    .unwrap();
                    app.run_until_parked();
                }
                best = best.min(cpu_time() - start);
            }
            let per_frame = best / (FRAMES / 8) as u32;
            match style {
                None => {
                    baseline = per_frame;
                    println!("a frame of the window with one style D cell: {per_frame:?}");
                }
                Some(ix) => {
                    let per_cell =
                        per_frame.saturating_sub(baseline).as_secs_f64() * 1e6 / COPIES as f64;
                    println!(
                        "{} {:<28} {:>7.1} us a frame for each cell ({})",
                        STYLES[ix].letter, STYLES[ix].name, per_cell, STYLES[ix].cost
                    );
                }
            }
        }
    }

    /// A window's root that keeps its content, as the app's panes are kept.
    struct Kept(Entity<Copies>);

    impl Render for Kept {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.0
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }
}
