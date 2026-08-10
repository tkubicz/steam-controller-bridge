//! The versioned, data-driven guided mouse measurement sequence.

use std::time::Duration;

#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "protocol markers are emitted only by macOS capture but parsed portably"
    )
)]
pub(crate) const GUIDED_PROTOCOL: &str = "lizard-guided-v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PadPoint {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl PadPoint {
    pub(crate) const ALL: [Self; 5] = [
        Self::Center,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::TopLeft => "top-left corner",
            Self::TopRight => "top-right corner",
            Self::BottomLeft => "bottom-left corner",
            Self::BottomRight => "bottom-right corner",
        }
    }

    pub(crate) const fn normalized(self) -> (f32, f32) {
        match self {
            Self::Center => (0.5, 0.5),
            Self::TopLeft => (0.18, 0.18),
            Self::TopRight => (0.82, 0.18),
            Self::BottomLeft => (0.18, 0.82),
            Self::BottomRight => (0.82, 0.82),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
    TopLeftToBottomRight,
    BottomRightToTopLeft,
    TopRightToBottomLeft,
    BottomLeftToTopRight,
}

impl Direction {
    pub(crate) const ALL: [Self; 8] = [
        Self::LeftToRight,
        Self::RightToLeft,
        Self::TopToBottom,
        Self::BottomToTop,
        Self::TopLeftToBottomRight,
        Self::BottomRightToTopLeft,
        Self::TopRightToBottomLeft,
        Self::BottomLeftToTopRight,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::LeftToRight => "left_to_right",
            Self::RightToLeft => "right_to_left",
            Self::TopToBottom => "top_to_bottom",
            Self::BottomToTop => "bottom_to_top",
            Self::TopLeftToBottomRight => "top_left_to_bottom_right",
            Self::BottomRightToTopLeft => "bottom_right_to_top_left",
            Self::TopRightToBottomLeft => "top_right_to_bottom_left",
            Self::BottomLeftToTopRight => "bottom_left_to_top_right",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::LeftToRight => "left to right",
            Self::RightToLeft => "right to left",
            Self::TopToBottom => "top to bottom",
            Self::BottomToTop => "bottom to top",
            Self::TopLeftToBottomRight => "top-left to bottom-right",
            Self::BottomRightToTopLeft => "bottom-right to top-left",
            Self::TopRightToBottomLeft => "top-right to bottom-left",
            Self::BottomLeftToTopRight => "bottom-left to top-right",
        }
    }

    pub(crate) const fn endpoints(self) -> ((f32, f32), (f32, f32)) {
        match self {
            Self::LeftToRight => ((0.12, 0.5), (0.88, 0.5)),
            Self::RightToLeft => ((0.88, 0.5), (0.12, 0.5)),
            Self::TopToBottom => ((0.5, 0.12), (0.5, 0.88)),
            Self::BottomToTop => ((0.5, 0.88), (0.5, 0.12)),
            Self::TopLeftToBottomRight => ((0.16, 0.16), (0.84, 0.84)),
            Self::BottomRightToTopLeft => ((0.84, 0.84), (0.16, 0.16)),
            Self::TopRightToBottomLeft => ((0.84, 0.16), (0.16, 0.84)),
            Self::BottomLeftToTopRight => ((0.16, 0.84), (0.84, 0.16)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwipeSpeed {
    Slow,
    Fast,
}

impl SwipeSpeed {
    const ALL: [Self; 2] = [Self::Slow, Self::Fast];

    const fn id(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Fast => "fast",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Slow => "Slow",
            Self::Fast => "Fast",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrialVisual {
    Hold(PadPoint),
    Swipe(Direction, SwipeSpeed),
    Precision(PadPoint),
    Click(PadPoint),
    ClickDrag(Direction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuidedTrial {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) instruction: String,
    pub(crate) duration: Duration,
    pub(crate) visual: TrialVisual,
}

pub(crate) fn guided_trials() -> Vec<GuidedTrial> {
    let mut trials = Vec::with_capacity(39);
    for point in PadPoint::ALL {
        trials.push(GuidedTrial {
            id: format!("hold_{}", point.id()),
            title: format!("Stationary hold: {}", point.label()),
            instruction: format!(
                "Place one finger at the {} and keep it as still as possible.",
                point.label()
            ),
            duration: Duration::from_secs(4),
            visual: TrialVisual::Hold(point),
        });
    }
    for speed in SwipeSpeed::ALL {
        for direction in Direction::ALL {
            trials.push(GuidedTrial {
                id: format!("swipe_{}_{}", speed.id(), direction.id()),
                title: format!("{} swipe: {}", speed.label(), direction.label()),
                instruction: format!(
                    "Swipe {} {} across the pad once, then lift your finger.",
                    speed.id(),
                    direction.label()
                ),
                duration: Duration::from_secs(3),
                visual: TrialVisual::Swipe(direction, speed),
            });
        }
    }
    for point in PadPoint::ALL {
        trials.push(GuidedTrial {
            id: format!("precision_{}", point.id()),
            title: format!("Precision motion: {}", point.label()),
            instruction: format!(
                "Make tiny controlled circles around the {} without clicking.",
                point.label()
            ),
            duration: Duration::from_secs(4),
            visual: TrialVisual::Precision(point),
        });
    }
    for point in PadPoint::ALL {
        trials.push(GuidedTrial {
            id: format!("click_{}", point.id()),
            title: format!("Clicks: {}", point.label()),
            instruction: format!(
                "Keep the pointer still and click three times at the {}.",
                point.label()
            ),
            duration: Duration::from_secs(6),
            visual: TrialVisual::Click(point),
        });
    }
    for direction in Direction::ALL {
        trials.push(GuidedTrial {
            id: format!("click_drag_{}", direction.id()),
            title: format!("Click-drag: {}", direction.label()),
            instruction: format!(
                "Press the pad, drag {} while held, then release.",
                direction.label()
            ),
            duration: Duration::from_secs(4),
            visual: TrialVisual::ClickDrag(direction),
        });
    }
    debug_assert_eq!(trials.len(), 39);
    trials
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn thorough_protocol_has_unique_stable_trials() {
        let trials = guided_trials();
        assert_eq!(trials.len(), 39);
        assert_eq!(
            trials
                .iter()
                .map(|trial| &trial.id)
                .collect::<BTreeSet<_>>()
                .len(),
            trials.len()
        );
        assert_eq!(trials.first().unwrap().id, "hold_center");
        assert_eq!(
            trials.last().unwrap().id,
            "click_drag_bottom_left_to_top_right"
        );
    }
}
