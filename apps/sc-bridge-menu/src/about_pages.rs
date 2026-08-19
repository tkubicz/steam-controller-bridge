use eframe::egui;
use semver::Version;
use ui_theme::{ACCENT, MUTED_TEXT, ON_ACCENT, SUCCESS, TEXT};

use crate::window_ui::{
    full_width_card, load_texture, parse_release_notes, render_inline, render_release_sections,
    ReleaseNotes,
};

const REPOSITORY_URL: &str = "https://github.com/tkubicz/steam-controller-bridge";
const CHANGELOG_URL: &str =
    "https://github.com/tkubicz/steam-controller-bridge/blob/main/CHANGELOG.md";
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const GITHUB_MARK: &[u8] = include_bytes!("../../../packaging/macos/GitHubMark.png");

/// Static About and Changelog content shared by the tabbed application window.
pub(crate) struct AboutContent {
    github_mark: egui::TextureHandle,
    releases: Vec<ReleaseNotes>,
    running_version: String,
}

impl AboutContent {
    pub(crate) fn new(ctx: &egui::Context, running_version: &Version) -> Self {
        Self {
            github_mark: load_texture(ctx, "github-mark", GITHUB_MARK),
            releases: parse_release_notes(CHANGELOG),
            running_version: running_version.to_string(),
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
                        if let Err(error) = menu_shell::open_url(REPOSITORY_URL) {
                            eprintln!("cannot open project repository: {error}");
                        }
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
                    if let Err(error) = menu_shell::open_url(CHANGELOG_URL) {
                        eprintln!("cannot open project changelog: {error}");
                    }
                }
            });
        });
        ui.add_space(10.0);

        for (index, release) in self.releases.iter().enumerate() {
            full_width_card(ui, 20, |ui| {
                let title = release.title.plain();
                let is_current = title.starts_with(&self.running_version);
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
        assert!(current
            .title
            .plain()
            .starts_with(&crate::update_check::running_version().to_string()));
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
    fn every_release_title_link_targets_its_version_tag() {
        let releases = parse_release_notes(CHANGELOG);
        for release in releases {
            let title = release.title.plain();
            let version = title.split_whitespace().next().expect("release version");
            let url = release
                .title
                .spans
                .first()
                .and_then(|span| span.url.as_deref())
                .expect("release title link");
            assert!(
                url.ends_with(&format!("/v{version}")) || url.ends_with(&format!("...v{version}")),
                "unexpected release target: {url}"
            );
        }
    }
}
