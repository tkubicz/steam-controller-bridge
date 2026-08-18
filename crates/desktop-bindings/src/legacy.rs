//! Reading binding stores written before pad regions existed.
//!
//! A frozen mirror of the version 1-4 schema. It must not drift with
//! `crate::model`: a change there is a change to version 5, not to what version
//! 4 meant.

use serde::Deserialize;

use crate::model::{
    BindingAction, BindingProfile, ControlBindings, PadBindings, PadConfig, PadFeedbackConfig,
    PadMotionMode, PadRegion, BINDINGS_VERSION, DEFAULT_PAD_SPEED_PERCENT,
};
use crate::store::BindingStore;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyStore {
    #[allow(
        dead_code,
        reason = "the caller already read the version; this field exists so \
                  `deny_unknown_fields` still accepts the document"
    )]
    version: u32,
    profiles: Vec<LegacyProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProfile {
    id: String,
    name: String,
    #[serde(default)]
    bindings: LegacyBindings,
    #[serde(default)]
    pads: LegacyPads,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyBindings {
    l4: Option<BindingAction>,
    l5: Option<BindingAction>,
    r4: Option<BindingAction>,
    r5: Option<BindingAction>,
    quick_access: Option<BindingAction>,
    left_pad_click: Option<BindingAction>,
    right_pad_click: Option<BindingAction>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyPads {
    right_mouse: LegacyPointerPad,
    left_scroll: LegacyScrollPad,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyPointerPad {
    enabled: bool,
    feedback: PadFeedbackConfig,
    speed_percent: u16,
}

impl Default for LegacyPointerPad {
    fn default() -> Self {
        Self {
            enabled: false,
            feedback: PadFeedbackConfig::default(),
            speed_percent: DEFAULT_PAD_SPEED_PERCENT,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyScrollPad {
    enabled: bool,
    feedback: PadFeedbackConfig,
    speed_percent: u16,
    momentum: bool,
}

impl Default for LegacyScrollPad {
    fn default() -> Self {
        Self {
            enabled: false,
            feedback: PadFeedbackConfig::default(),
            speed_percent: DEFAULT_PAD_SPEED_PERCENT,
            momentum: true,
        }
    }
}

/// Decodes a version 1 to 4 document. The caller validates the result and owes
/// the file a rewrite.
///
/// # Errors
/// Returns a descriptive error when the document cannot be decoded.
pub(crate) fn parse_pre_region_store(bytes: &[u8], version: u32) -> Result<BindingStore, String> {
    let legacy: LegacyStore = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid version {version} bindings JSON: {error}"))?;
    Ok(BindingStore {
        version: BINDINGS_VERSION,
        profiles: legacy.profiles.into_iter().map(migrate_profile).collect(),
    })
}

fn migrate_profile(profile: LegacyProfile) -> BindingProfile {
    let LegacyProfile {
        id,
        name,
        bindings,
        pads,
    } = profile;
    BindingProfile {
        id,
        name,
        bindings: ControlBindings {
            l4: bindings.l4,
            l5: bindings.l5,
            r4: bindings.r4,
            r5: bindings.r5,
            quick_access: bindings.quick_access,
        },
        pads: PadBindings {
            left: PadConfig {
                motion: if pads.left_scroll.enabled {
                    PadMotionMode::Scroll
                } else {
                    PadMotionMode::None
                },
                speed_percent: pads.left_scroll.speed_percent,
                momentum: pads.left_scroll.momentum,
                feedback: pads.left_scroll.feedback,
                regions: whole_pad_click(bindings.left_pad_click),
            },
            right: PadConfig {
                motion: if pads.right_mouse.enabled {
                    PadMotionMode::Pointer
                } else {
                    PadMotionMode::None
                },
                speed_percent: pads.right_mouse.speed_percent,
                // The pointer pad had no momentum setting; the current default
                // applies if the user later switches this pad to scrolling.
                momentum: true,
                feedback: pads.right_mouse.feedback,
                regions: whole_pad_click(bindings.right_pad_click),
            },
        },
    }
}

/// A pre-region pad click is one whole-pad region; an unbound one is no
/// regions at all.
fn whole_pad_click(action: Option<BindingAction>) -> Vec<PadRegion> {
    let Some(action) = action else {
        return Vec::new();
    };
    let mut regions = PadRegion::whole();
    regions[0].click = Some(action);
    regions
}
