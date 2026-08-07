//! The one palette every GUI surface in the workspace draws from.
//!
//! Lives in its own crate because the surfaces that need it sit in different
//! binaries: the menu app's bindings editor and profile overlay, and the
//! standalone visualizer. A module inside one of those binaries cannot be
//! reached from the others, which is how the visualizer ended up rendering in
//! stock egui colours.
//!
//! `tools/make-app-icon.py` mirrors three of these values by hand. Python
//! cannot read Rust constants, so changing `ACCENT`, `SURFACE_RAISED` or
//! `SUNKEN` means rerunning that script.

use eframe::egui;

/// Cyan. Selection fills, focus strokes, primary buttons, the active profile.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 211, 224);
/// Accent at low emphasis: a selected element that should not shout.
pub const ACCENT_SUBTLE: egui::Color32 = egui::Color32::from_rgb(34, 67, 73);
/// Foreground for anything sitting on [`ACCENT`].
pub const ON_ACCENT: egui::Color32 = egui::Color32::from_rgb(8, 28, 32);

/// Default window and body surface.
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(29, 34, 42);
/// One step up from [`SURFACE`]: inactive widgets, cards, unselected chips.
pub const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(36, 42, 51);
/// One step down from [`SURFACE`]: the drawing canvas, the darkest plane.
pub const SUNKEN: egui::Color32 = egui::Color32::from_rgb(18, 21, 26);
/// Panel background, between [`SUNKEN`] and [`SURFACE`].
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(14, 17, 21);
/// Recessed fills: text entry, stick wells, button caps.
pub const INSET: egui::Color32 = egui::Color32::from_rgb(22, 26, 32);

/// Hairline borders between planes.
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(46, 52, 62);
/// Widget background under the pointer.
pub const HOVERED: egui::Color32 = egui::Color32::from_rgb(45, 54, 65);
/// Widget background while pressed.
pub const ACTIVE: egui::Color32 = egui::Color32::from_rgb(50, 61, 73);

/// Primary body text.
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(232, 237, 243);
/// Labels, captions, anything secondary.
pub const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(157, 166, 177);
/// Strong strokes on illustrations: the controller silhouette.
pub const OUTLINE: egui::Color32 = egui::Color32::from_rgb(126, 136, 149);
/// Weak strokes on illustrations: interior detail, axes, unselected controls.
pub const DETAIL: egui::Color32 = egui::Color32::from_rgb(82, 92, 105);

/// Affirmative status. Deliberately not [`ACCENT`], which already carries
/// selection and would make "connected" indistinguishable from "chosen".
pub const SUCCESS: egui::Color32 = egui::Color32::from_rgb(95, 211, 155);
/// Errors and disconnected states.
pub const DANGER: egui::Color32 = egui::Color32::from_rgb(255, 115, 115);

/// Apply the palette to an egui context.
///
/// Call once per surface during setup. Anything drawn with an explicit colour
/// still needs the constants above; this only covers what egui paints itself.
pub fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = INSET;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ON_ACCENT);
    visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
    visuals.widgets.hovered.bg_fill = HOVERED;
    visuals.widgets.active.bg_fill = ACTIVE;
    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::{
        configure_visuals, ACCENT, ACTIVE, HOVERED, INSET, ON_ACCENT, PANEL, SURFACE,
        SURFACE_RAISED,
    };
    use eframe::egui;

    #[test]
    fn configure_visuals_applies_the_palette() {
        let ctx = egui::Context::default();
        configure_visuals(&ctx);

        // `set_visuals` writes into the style for the *active* theme, so the
        // read-back has to select the same one rather than assume dark.
        let visuals = ctx.style_of(ctx.theme()).visuals.clone();
        assert!(visuals.dark_mode);
        assert_eq!(visuals.panel_fill, PANEL);
        assert_eq!(visuals.window_fill, SURFACE);
        assert_eq!(visuals.extreme_bg_color, INSET);
        assert_eq!(visuals.selection.bg_fill, ACCENT);
        assert_eq!(visuals.selection.stroke.color, ON_ACCENT);
        assert_eq!(visuals.widgets.inactive.bg_fill, SURFACE_RAISED);
        assert_eq!(visuals.widgets.hovered.bg_fill, HOVERED);
        assert_eq!(visuals.widgets.active.bg_fill, ACTIVE);
    }

    /// The surfaces have to stay distinguishable, or the depth the layout
    /// relies on collapses into one flat plane.
    #[test]
    fn the_surface_ramp_is_ordered_darkest_first() {
        let luminance = |colour: egui::Color32| {
            u32::from(colour.r()) + u32::from(colour.g()) + u32::from(colour.b())
        };
        let ramp = [
            super::PANEL,
            super::SUNKEN,
            super::INSET,
            SURFACE,
            SURFACE_RAISED,
        ];
        for pair in ramp.windows(2) {
            assert!(
                luminance(pair[0]) < luminance(pair[1]),
                "{:?} should be darker than {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}
