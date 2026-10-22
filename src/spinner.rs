//! The spinner that marks running work.
//!
//! The running line of a reply, the sidebar's mark for a working session and the workers panel's
//! mark for a running worker all use it, so they look alike and turn together. The look is the
//! kit's spoke spinner, easing in and out once in each turn.

use std::time::Duration;

use gpui_kit::component::spinner::Spinner;
use gpui_kit::ease_in_out;

/// How long one turn takes.
pub const TURN: Duration = Duration::from_millis(1200);

/// The spinner of running work. The caller sets its size and color.
pub fn spinner() -> Spinner {
    Spinner::new().ease(ease_in_out).duration(TURN)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spokes of the icon are eight alike, so a step of more than 22.5 degrees in one
    /// frame looks like a step backwards. The fastest part of the turn must stay under that
    /// at 30 frames a second, the lowest rate a display or a throttled window is likely to
    /// give. A slower easing or a longer turn passes, a faster one fails.
    #[test]
    fn the_fastest_step_of_a_turn_does_not_look_backwards() {
        let frames = (TURN.as_secs_f32() * 30.).round() as usize;
        let angle = |frame: usize| ease_in_out(frame as f32 / frames as f32) * 360.;
        let widest = (1..=frames)
            .map(|frame| angle(frame) - angle(frame - 1))
            .fold(0., f32::max);
        assert!(widest < 22.5, "{widest} degrees in one frame at 30 fps");
    }
}
