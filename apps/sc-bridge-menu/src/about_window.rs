use eframe::egui;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use ui_theme::{ACCENT, BORDER, MUTED_TEXT, ON_ACCENT, PANEL, SURFACE, SURFACE_RAISED, TEXT};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

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

#[derive(Debug, PartialEq, Eq)]
struct ReleaseNotes {
    title: InlineText,
    sections: Vec<ReleaseSection>,
}

#[derive(Debug, PartialEq, Eq)]
struct ReleaseSection {
    title: String,
    notes: Vec<InlineText>,
}

#[derive(Debug, PartialEq, Eq)]
struct InlineText {
    spans: Vec<InlineSpan>,
}

#[derive(Debug, PartialEq, Eq)]
struct InlineSpan {
    text: String,
    url: Option<String>,
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
            configure_about_style(&creation.egui_ctx);
            activate_for_about();
            Ok(Box::new(AboutApp::new(&creation.egui_ctx)))
        }),
    )
    .map_err(|error| error.to_string())
}

fn configure_about_style(ctx: &egui::Context) {
    ui_theme::configure_ui(ctx);
    let theme = ctx.theme();
    let mut style = (*ctx.style_of(theme)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    ctx.set_style_of(theme, style);
}

#[allow(deprecated)] // `activate()` requires macOS 14; the app supports macOS 13.
fn activate_for_about() {
    let mtm = MainThreadMarker::new().expect("eframe starts on the main thread");
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
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
            releases: parse_changelog(CHANGELOG),
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

    fn hero_transition(ui: &mut egui::Ui) {
        const HEIGHT: f32 = 28.0;
        const STEPS: u8 = 14;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), HEIGHT),
            egui::Sense::hover(),
        );
        let start = egui::Rgba::from(SURFACE);
        let end = egui::Rgba::from(PANEL);
        for step in 0..STEPS {
            let top = f32::from(step) / f32::from(STEPS);
            let bottom = f32::from(step + 1) / f32::from(STEPS);
            let strip = egui::Rect::from_min_max(
                egui::pos2(rect.left(), egui::lerp(rect.top()..=rect.bottom(), top)),
                egui::pos2(rect.right(), egui::lerp(rect.top()..=rect.bottom(), bottom)),
            );
            ui.painter().rect_filled(
                strip,
                0.0,
                egui::Color32::from(egui::lerp(start..=end, bottom)),
            );
        }
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
                    Self::hero_transition(ui);
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

fn full_width_card<R>(
    ui: &mut egui::Ui,
    padding: i8,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let inner_width = (ui.available_width() - f32::from(padding) * 2.0 - 2.0).max(0.0);
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(padding))
        .show(ui, |ui| {
            ui.set_width(inner_width);
            add_contents(ui)
        })
}

fn load_texture(ctx: &egui::Context, name: &str, png: &[u8]) -> egui::TextureHandle {
    let icon = eframe::icon_data::from_png_bytes(png).expect("embedded PNG asset is valid");
    let size = [icon.width as usize, icon.height as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
    ctx.load_texture(
        name,
        image,
        egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
    )
}

fn parse_changelog(markdown: &str) -> Vec<ReleaseNotes> {
    let mut releases = Vec::new();
    let mut release: Option<ReleaseNotes> = None;
    let mut section_index: Option<usize> = None;

    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(previous) = release.take() {
                releases.push(previous);
            }
            release = Some(ReleaseNotes {
                title: parse_inline(title),
                sections: Vec::new(),
            });
            section_index = None;
        } else if let Some(title) = line.strip_prefix("### ") {
            let Some(current) = release.as_mut() else {
                continue;
            };
            current.sections.push(ReleaseSection {
                title: parse_inline(title).plain(),
                notes: Vec::new(),
            });
            section_index = Some(current.sections.len() - 1);
        } else if line.starts_with("* ") || line.starts_with("- ") {
            let Some(current) = release.as_mut() else {
                continue;
            };
            let index = *section_index.get_or_insert_with(|| {
                current.sections.push(ReleaseSection {
                    title: "Overview".to_owned(),
                    notes: Vec::new(),
                });
                current.sections.len() - 1
            });
            current.sections[index].notes.push(parse_inline(&line[2..]));
        }
    }
    if let Some(last) = release {
        releases.push(last);
    }
    releases
}

fn parse_inline(markdown: &str) -> InlineText {
    let mut spans = Vec::new();
    let mut rest = markdown;
    while let Some(open) = rest.find('[') {
        push_span(&mut spans, &rest[..open], None);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find("](") else {
            push_span(&mut spans, &rest[open..], None);
            rest = "";
            break;
        };
        let after_label = &after_open[close + 2..];
        let Some(end) = after_label.find(')') else {
            push_span(&mut spans, &rest[open..], None);
            rest = "";
            break;
        };
        push_span(
            &mut spans,
            &after_open[..close],
            Some(after_label[..end].to_owned()),
        );
        rest = &after_label[end + 1..];
    }
    push_span(&mut spans, rest, None);
    InlineText { spans }
}

fn push_span(spans: &mut Vec<InlineSpan>, text: &str, url: Option<String>) {
    let text = text.replace("**", "").replace('`', "");
    if !text.is_empty() {
        spans.push(InlineSpan { text, url });
    }
}

fn render_inline(ui: &mut egui::Ui, inline: &InlineText, size: f32, strong: bool) {
    for span in &inline.spans {
        let mut text = egui::RichText::new(&span.text).size(size);
        if strong {
            text = text.strong();
        }
        if let Some(url) = &span.url {
            ui.hyperlink_to(text.color(ACCENT), url);
        } else {
            ui.label(text.color(TEXT));
        }
    }
}

impl InlineText {
    fn plain(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_changelog_contains_the_current_release() {
        let releases = parse_changelog(CHANGELOG);
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
        let releases = parse_changelog(CHANGELOG);
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
        let inline = parse_inline("**menu:** fix it ([#19](https://example.test/19))");
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
