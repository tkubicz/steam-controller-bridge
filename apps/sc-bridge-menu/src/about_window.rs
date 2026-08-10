use eframe::egui;
use ui_theme::{ACCENT, MUTED_TEXT, ON_ACCENT, SUCCESS, TEXT};

use crate::window_ui::{
    full_width_card, load_texture, parse_release_notes, render_inline, render_release_sections,
    ReleaseNotes,
};

const REPOSITORY_URL: &str = "https://github.com/tkubicz/steam-controller-bridge";
const CHANGELOG_URL: &str =
    "https://github.com/tkubicz/steam-controller-bridge/blob/main/CHANGELOG.md";
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const BUNDLE_VERSION: &str = include_str!("../../../version.txt");
const GITHUB_MARK: &[u8] = include_bytes!("../../../packaging/macos/GitHubMark.png");

/// Static About and Changelog content shared by the tabbed application window.
pub(crate) struct AboutContent {
    github_mark: egui::TextureHandle,
    releases: Vec<ReleaseNotes>,
}

impl AboutContent {
    pub(crate) fn new(ctx: &egui::Context) -> Self {
        Self {
            github_mark: load_texture(ctx, "github-mark", GITHUB_MARK),
            releases: parse_release_notes(CHANGELOG),
        }
    }

    pub(crate) fn about_page(&self, ui: &mut egui::Ui) {
        full_width_card(ui, 24, |ui| {
            ui.label(
                egui::RichText::new("Your controller, translated.")
                    .size(22.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Steam Controller Bridge connects a Steam Controller 2 to macOS as a standard USB Xbox gamepad, without requiring Steam to be running.",
                )
                .size(16.0)
                .line_height(Some(23.0))
                .color(TEXT),
            );
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(
                    "It also provides configurable desktop profiles, trackpad mouse and scrolling support, haptics, and a controller-driven profile wheel.",
                )
                .color(MUTED_TEXT),
            );
        });

        ui.add_space(14.0);
        full_width_card(ui, 20, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Open source").strong().color(TEXT));
                    ui.label(
                        egui::RichText::new("Source code, releases, and issue tracker")
                            .color(MUTED_TEXT),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let image = egui::Image::new(&self.github_mark)
                        .fit_to_exact_size(egui::vec2(18.0, 18.0));
                    if ui
                        .add(
                            egui::Button::image_and_text(
                                image,
                                egui::RichText::new("View on GitHub  ↗")
                                    .strong()
                                    .color(ON_ACCENT),
                            )
                            .fill(ACCENT)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(8),
                        )
                        .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(REPOSITORY_URL));
                    }
                });
            });
        });
    }

    pub(crate) fn changelog_page(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Release notes")
                        .size(22.0)
                        .strong()
                        .color(TEXT),
                );
                ui.label(
                    egui::RichText::new("What changed in each published version").color(MUTED_TEXT),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.link("Full changelog ↗").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(CHANGELOG_URL));
                }
            });
        });
        ui.add_space(10.0);

        for (index, release) in self.releases.iter().enumerate() {
            full_width_card(ui, 20, |ui| {
                let title = release.title.plain();
                let is_current = title.starts_with(BUNDLE_VERSION.trim());
                let id = ui.make_persistent_id(("changelog-release", title.as_str()));
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    index == 0,
                )
                .show_header(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    render_inline(ui, &release.title, 19.0, true);
                    if is_current {
                        ui.label(
                            egui::RichText::new("  Current")
                                .size(12.0)
                                .strong()
                                .color(SUCCESS),
                        );
                    }
                })
                .body(|ui| render_release_sections(ui, release));
            });
            ui.add_space(12.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_changelog_contains_the_current_release() {
        let releases = parse_release_notes(CHANGELOG);
        let current = releases.first().expect("at least one release");
        assert!(current.title.plain().starts_with(BUNDLE_VERSION.trim()));
        assert!(!current.sections.is_empty());
        assert!(current
            .sections
            .iter()
            .all(|section| !section.notes.is_empty()));
    }

    #[test]
    fn github_mark_decodes_as_png() {
        let image = eframe::icon_data::from_png_bytes(GITHUB_MARK)
            .expect("GitHubMark must stay a valid PNG");
        assert!(image.width > 0 && image.height > 0);
    }

    #[test]
    fn first_release_uses_the_same_format_as_generated_releases() {
        let releases = parse_release_notes(CHANGELOG);
        let first = releases
            .iter()
            .find(|release| release.title.plain().starts_with("1.0.0"))
            .expect("1.0.0 release");

        assert_eq!(first.title.plain(), "1.0.0 (2026-07-30)");
        assert_eq!(
            first
                .title
                .spans
                .first()
                .and_then(|span| span.url.as_deref()),
            Some("https://github.com/tkubicz/steam-controller-bridge/releases/tag/v1.0.0")
        );
        assert!(first
            .sections
            .iter()
            .any(|section| section.title == "Features"));
    }

    #[test]
    fn cards_share_the_available_width_regardless_of_padding_or_content() {
        for width in [540.0, 700.0] {
            egui::__run_test_ui(|ui| {
                ui.set_width(width);
                let description = full_width_card(ui, 24, |ui| ui.label("Longer description"));
                let action = full_width_card(ui, 20, |ui| ui.label("Short"));
                assert!(
                    (description.response.rect.width() - action.response.rect.width()).abs() < 0.1
                );
                assert!((description.response.rect.width() - width).abs() < 0.1);
            });
        }
    }
}
