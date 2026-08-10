use eframe::egui;
use ui_theme::{ACCENT, MUTED_TEXT, ON_ACCENT, PANEL, SURFACE, TEXT};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::window_ui::{
    activate_window, configure_window_style, full_width_card, hero_transition, load_texture,
    parse_release_notes, render_inline, ReleaseNotes,
};

const WINDOW_TITLE: &str = "About Steam Controller Bridge";
const WINDOW_SIZE: [f32; 2] = [720.0, 620.0];
const MIN_WINDOW_SIZE: [f32; 2] = [620.0, 520.0];
const REPOSITORY_URL: &str = "https://github.com/tkubicz/steam-controller-bridge";
const CHANGELOG_URL: &str =
    "https://github.com/tkubicz/steam-controller-bridge/blob/main/CHANGELOG.md";
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const BUNDLE_VERSION: &str = include_str!("../../../version.txt");
const APP_ICON: &[u8] = include_bytes!("../../../packaging/macos/AppIcon.png");
const GITHUB_MARK: &[u8] = include_bytes!("../../../packaging/macos/GitHubMark.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    About,
    Changelog,
}

pub fn run() -> Result<(), String> {
    let icon = eframe::icon_data::from_png_bytes(APP_ICON).map_err(|error| error.to_string())?;
    let options = eframe::NativeOptions {
        // Keep the About process consistent with the menu-bar application: it
        // can become key without acquiring a persistent Dock icon.
        event_loop_builder: Some(Box::new(|builder| {
            builder
                .with_activation_policy(ActivationPolicy::Accessory)
                .with_activate_ignoring_other_apps(true);
        })),
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE)
            .with_icon(icon),
        ..eframe::NativeOptions::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(|creation| {
            configure_window_style(&creation.egui_ctx);
            activate_window();
            Ok(Box::new(AboutApp::new(&creation.egui_ctx)))
        }),
    )
    .map_err(|error| error.to_string())
}

struct AboutApp {
    page: Page,
    app_icon: egui::TextureHandle,
    github_mark: egui::TextureHandle,
    releases: Vec<ReleaseNotes>,
}

impl AboutApp {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            page: Page::About,
            app_icon: load_texture(ctx, "app-icon", APP_ICON),
            github_mark: load_texture(ctx, "github-mark", GITHUB_MARK),
            releases: parse_release_notes(CHANGELOG),
        }
    }

    fn hero(&self, ui: &mut egui::Ui) {
        let content_width = (ui.available_width() - 56.0).max(0.0);
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::symmetric(28, 24))
            .show(ui, |ui| {
                ui.set_min_width(content_width);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&self.app_icon)
                            .fit_to_exact_size(egui::vec2(84.0, 84.0))
                            .corner_radius(20),
                    );
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Steam Controller Bridge")
                                .size(28.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new("A native bridge for Steam Controller 2 on macOS")
                                .size(15.0)
                                .color(MUTED_TEXT),
                        );
                        ui.add_space(7.0);
                        egui::Frame::new()
                            .fill(ui_theme::ACCENT_SUBTLE)
                            .corner_radius(7)
                            .inner_margin(egui::Margin::symmetric(10, 5))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Version {}",
                                        BUNDLE_VERSION.trim()
                                    ))
                                    .size(13.0)
                                    .color(ACCENT),
                                );
                            });
                    });
                });
            });
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.page, Page::About, "About");
            ui.selectable_value(&mut self.page, Page::Changelog, "Changelog");
        });
    }

    fn about_page(&self, ui: &mut egui::Ui) {
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

    fn changelog_page(&self, ui: &mut egui::Ui) {
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
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for release in &self.releases {
                    full_width_card(ui, 20, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            render_inline(ui, &release.title, 19.0, true);
                        });
                        for section in &release.sections {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(section.title.to_uppercase())
                                    .size(11.0)
                                    .strong()
                                    .color(ACCENT),
                            );
                            for note in &section.notes {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 3.0;
                                    ui.label(egui::RichText::new("•").color(ACCENT));
                                    render_inline(ui, note, 14.0, false);
                                });
                            }
                        }
                    });
                    ui.add_space(12.0);
                }
            });
    }
}

impl eframe::App for AboutApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::new().fill(PANEL).show(ui, |ui| {
            ui.set_min_size(ui.available_size());

            egui::Panel::bottom("about-footer")
                .exact_size(36.0)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(PANEL))
                .show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("Copyright © Lynxware · MIT licensed")
                                .size(12.0)
                                .color(MUTED_TEXT),
                        );
                    });
                });

            egui::Panel::top("about-hero")
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(PANEL))
                .show(ui, |ui| {
                    // These three surfaces deliberately touch: the gradient is
                    // the transition. Egui's normal vertical item gap would
                    // expose PANEL as a solid black stripe between them.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    self.hero(ui);
                    hero_transition(ui);
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        self.navigation(ui);
                    });
                    ui.add_space(4.0);
                    ui.separator();
                });

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(24, 14)),
                )
                .show(ui, |ui| match self.page {
                    Page::About => self.about_page(ui),
                    Page::Changelog => self.changelog_page(ui),
                });
        });
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
        // A fix-only release has no Features section, so only require that
        // some section exists and none renders as an empty card.
        assert!(!current.sections.is_empty());
        assert!(current
            .sections
            .iter()
            .all(|section| !section.notes.is_empty()));
    }

    #[test]
    fn embedded_images_decode_as_png() {
        for (name, bytes) in [("AppIcon", APP_ICON), ("GitHubMark", GITHUB_MARK)] {
            let image = eframe::icon_data::from_png_bytes(bytes)
                .unwrap_or_else(|error| panic!("{name} must stay a valid PNG: {error}"));
            assert!(image.width > 0 && image.height > 0);
        }
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
        assert!(first.sections.iter().all(|section| {
            section.title != "Added"
                && !section.notes.is_empty()
                && section.notes.iter().all(|note| !note.plain().is_empty())
        }));
    }

    #[test]
    fn markdown_links_remain_readable_and_clickable() {
        let inline = crate::window_ui::parse_release_notes(
            "## Example\n### Fixes\n* **menu:** fix it ([#19](https://example.test/19))",
        )
        .remove(0)
        .sections
        .remove(0)
        .notes
        .remove(0);
        assert_eq!(inline.plain(), "menu: fix it (#19)");
        let link = inline
            .spans
            .iter()
            .find(|span| span.url.is_some())
            .expect("link span");
        assert_eq!(link.text, "#19");
        assert_eq!(link.url.as_deref(), Some("https://example.test/19"));
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
