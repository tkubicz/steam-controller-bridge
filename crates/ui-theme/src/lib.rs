//! The one visual and typography setup every GUI surface in the workspace uses.
//!
//! Lives in its own crate because the surfaces that need it sit in different
//! binaries: the menu app's bindings editor and profile overlay, and the
//! standalone visualizer. A module inside one of those binaries cannot be
//! reached from the others, which previously let their palette, font, and text
//! rasterization settings drift apart.
//!
//! `tools/make-app-icon.py` mirrors three of these values by hand. Python
//! cannot read Rust constants, so changing `ACCENT`, `SURFACE_RAISED` or
//! `SUNKEN` means rerunning that script.

use eframe::egui;

const TEXT_COVERAGE_GAMMA: f32 = 0.7;

#[cfg(target_os = "macos")]
const SF_PRO_FONT_NAME: &str = "SF Pro";
#[cfg(target_os = "macos")]
const SF_PRO_FONT_PATH: &str = "/System/Library/Fonts/SFNS.ttf";
#[cfg(target_os = "macos")]
const SF_MONO_FONT_NAME: &str = "SF Mono";
#[cfg(target_os = "macos")]
const SF_MONO_FONT_PATH: &str = "/System/Library/Fonts/SFNSMono.ttf";

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

/// Apply the shared fonts, text rasterization, and palette to an egui context.
///
/// Call once per surface during setup. On macOS, SF Pro and SF Mono take
/// precedence while egui's bundled fonts remain as missing-glyph fallbacks.
/// Other platforms keep the bundled families. Anything drawn with an explicit
/// colour still needs the constants above; the palette only covers what egui
/// paints itself.
pub fn configure_ui(ctx: &egui::Context) {
    ctx.set_fonts(configured_font_definitions());

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = INSET;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ON_ACCENT);
    visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
    visuals.widgets.hovered.bg_fill = HOVERED;
    visuals.widgets.active.bg_fill = ACTIVE;
    // Egui's dark-mode default prioritizes maximum edge sharpness. A gentler
    // curve retains grayscale coverage around glyph edges, which more closely
    // matches Core Text on macOS and avoids stair-stepped diagonals.
    visuals.text_options.color_transfer_function =
        egui::epaint::FontColorTransferFunction::Gamma(TEXT_COVERAGE_GAMMA);
    ctx.set_visuals(visuals);
}

#[cfg(target_os = "macos")]
fn configured_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    for (name, path, family) in [
        (
            SF_PRO_FONT_NAME,
            SF_PRO_FONT_PATH,
            egui::FontFamily::Proportional,
        ),
        (
            SF_MONO_FONT_NAME,
            SF_MONO_FONT_PATH,
            egui::FontFamily::Monospace,
        ),
    ] {
        if let Err(error) = prepend_system_font(&mut fonts, name, path, family) {
            // Each GUI configures its context once, so this produces at most
            // one warning per missing family per process.
            eprintln!(
                "level=warn event=system_font_unavailable font={name:?} path={path:?} error={error:?}"
            );
        }
    }
    fonts
}

#[cfg(not(target_os = "macos"))]
fn configured_font_definitions() -> egui::FontDefinitions {
    egui::FontDefinitions::default()
}

#[cfg(any(target_os = "macos", test))]
fn prepend_system_font(
    fonts: &mut egui::FontDefinitions,
    name: &str,
    path: impl AsRef<std::path::Path>,
    family: egui::FontFamily,
) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    fonts.font_data.insert(
        name.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    let family_fonts = fonts.families.entry(family).or_default();
    family_fonts.retain(|font| font != name);
    family_fonts.insert(0, name.to_owned());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        configure_ui, configured_font_definitions, prepend_system_font, ACCENT, ACTIVE, HOVERED,
        INSET, ON_ACCENT, PANEL, SURFACE, SURFACE_RAISED, TEXT_COVERAGE_GAMMA,
    };
    use eframe::egui;

    #[test]
    fn configure_ui_applies_the_palette_and_text_rasterization() {
        let ctx = egui::Context::default();
        configure_ui(&ctx);

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
        assert_eq!(
            visuals.text_options.color_transfer_function,
            egui::epaint::FontColorTransferFunction::Gamma(TEXT_COVERAGE_GAMMA)
        );
    }

    #[test]
    fn configured_fonts_retain_every_bundled_fallback() {
        let defaults = egui::FontDefinitions::default();
        let configured = configured_font_definitions();

        for (family, bundled) in defaults.families {
            let configured_family = configured.families.get(&family).expect("font family");
            for font in bundled {
                assert!(
                    configured_family.contains(&font),
                    "bundled font {font:?} must remain in {family:?}"
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_fonts_precede_the_bundled_families() {
        let configured = configured_font_definitions();
        assert_eq!(
            configured.families[&egui::FontFamily::Proportional].first(),
            Some(&super::SF_PRO_FONT_NAME.to_owned())
        );
        assert_eq!(
            configured.families[&egui::FontFamily::Monospace].first(),
            Some(&super::SF_MONO_FONT_NAME.to_owned())
        );
    }

    #[test]
    fn a_missing_optional_system_font_leaves_the_defaults_unchanged() {
        let mut fonts = egui::FontDefinitions::default();
        let original_families = fonts.families.clone();
        let original_font_names: Vec<_> = fonts.font_data.keys().cloned().collect();
        let missing = std::env::temp_dir().join("sc-bridge-font-that-does-not-exist.ttf");

        assert!(prepend_system_font(
            &mut fonts,
            "missing-system-font",
            missing,
            egui::FontFamily::Proportional,
        )
        .is_err());
        assert_eq!(fonts.families, original_families);
        assert_eq!(
            fonts.font_data.keys().cloned().collect::<Vec<_>>(),
            original_font_names
        );
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
