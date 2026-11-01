//! Settings window. Runs as its own short-lived process (`--ui`) so the tray
//! process stays tiny when the window is closed.
//!
//! Layout: start-order list on the left, editor for the selected app on the
//! right. "Add app" swaps the editor for a searchable list of startup items.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Vec2};

use crate::config::{self, App, Config, StartupRef};
use crate::startup::{self, Item};
use crate::suggest::{self, Suggestion};

fn sugg_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(suggest::path()).and_then(|m| m.modified()).ok()
}

fn ago(t: u64) -> String {
    let s = suggest::now().saturating_sub(t);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

const ACCENT: Color32 = Color32::from_rgb(59, 130, 246);
const DANGER: Color32 = Color32::from_rgb(220, 80, 70);
const WARN: Color32 = Color32::from_rgb(217, 150, 40);

pub fn run() {
    let icon = egui::IconData { rgba: crate::tray::icon_rgba(), width: 32, height: 32 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("launchpad")
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([760.0, 460.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "launchpad",
        opts,
        Box::new(|cc| {
            theme(&cc.egui_ctx);
            Ok(Box::new(Ui::new()))
        }),
    );
}

fn theme(ctx: &egui::Context) {
    use egui::{FontId, TextStyle};
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 26.0;
        s.visuals.selection.bg_fill = ACCENT;
        s.visuals.hyperlink_color = ACCENT;
    });
}

#[derive(PartialEq)]
enum View {
    Edit,
    Add,
    Suggestions,
}

struct Ui {
    cfg: Config,
    saved: Config,
    items: Vec<Item>,
    log: String,
    status: Option<(String, bool)>, // (text, is_error)
    elevated: bool,
    selected: Option<usize>,
    view: View,
    search: String,
    show_log: bool,
    show_options: bool,
    /// Icon textures keyed by command line; None = no icon found.
    icons: HashMap<String, Option<egui::TextureHandle>>,
    suggestions: Vec<Suggestion>,
    sugg_mtime: Option<std::time::SystemTime>,
    last_poll: std::time::Instant,
}

impl Ui {
    fn new() -> Self {
        let (cfg, status) = match config::load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some((e, true))),
        };
        let view = if !suggest::load().is_empty() {
            View::Suggestions
        } else if cfg.apps.is_empty() {
            View::Add
        } else {
            View::Edit
        };
        Ui {
            selected: (!cfg.apps.is_empty()).then_some(0),
            saved: cfg.clone(),
            cfg,
            items: startup::scan(),
            log: read_log(),
            status,
            elevated: crate::win::is_elevated(),
            view,
            search: String::new(),
            show_log: false,
            show_options: false,
            icons: HashMap::new(),
            suggestions: suggest::load(),
            sugg_mtime: sugg_mtime(),
            last_poll: std::time::Instant::now(),
        }
    }

    /// Pick up suggestions filed while the window is open (checked every 2 s).
    fn poll_suggestions(&mut self, ctx: &egui::Context) {
        if self.last_poll.elapsed().as_secs_f32() >= 2.0 {
            self.last_poll = std::time::Instant::now();
            let m = sugg_mtime();
            if m != self.sugg_mtime {
                self.sugg_mtime = m;
                self.suggestions = suggest::load();
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }

    fn suggestions_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("Suggestions").heading().strong());
        hint(ui, "Filed by an assistant through launchpad's MCP server. Apply adds the changes as unsaved edits, so you can look them over before you Save.");
        ui.add_space(6.0);
        if self.suggestions.is_empty() {
            ui.label(RichText::new("No suggestions waiting.").weak());
            return;
        }
        let mut apply = None;
        let mut dismiss = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for s in &self.suggestions {
                egui::Frame::default()
                    .fill(ui.visuals().faint_bg_color)
                    .stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.5)))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(&s.summary).strong());
                                ui.label(RichText::new(format!("from {} · {}", s.source, ago(s.created))).small().weak());
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Dismiss").clicked() {
                                    dismiss = Some(s.id.clone());
                                }
                                if ui.add(primary_button("Apply")).clicked() {
                                    apply = Some(s.clone());
                                }
                            });
                        });
                        if !s.reason.is_empty() {
                            ui.label(&s.reason);
                        }
                        ui.add_space(4.0);
                        for op in &s.ops {
                            for (line, warn) in suggest::describe(&self.cfg, op) {
                                let t = RichText::new(line).monospace();
                                ui.label(if warn { t.color(WARN) } else { t });
                            }
                        }
                    });
                ui.add_space(6.0);
            }
        });

        if let Some(s) = apply {
            let mut c = self.cfg.clone();
            match suggest::apply(&mut c, &s.ops) {
                Ok(()) => {
                    self.cfg = c;
                    let _ = suggest::remove(&s.id);
                    self.suggestions.retain(|x| x.id != s.id);
                    self.status = Some(("Applied. Check the changes, then Save.".into(), false));
                    self.view = if self.suggestions.is_empty() { View::Edit } else { View::Suggestions };
                    if self.selected.is_none() && !self.cfg.apps.is_empty() {
                        self.selected = Some(0);
                    }
                }
                Err(e) => self.status = Some((format!("Can't apply: {e}"), true)),
            }
        }
        if let Some(id) = dismiss {
            let _ = suggest::remove(&id);
            self.suggestions.retain(|x| x.id != id);
        }
    }

    /// Icon for a command, loaded once and cached.
    fn icon(&mut self, ctx: &egui::Context, command: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.icons.get(command) {
            return t.clone();
        }
        let tex = crate::launch::icon_source(command)
            .and_then(|src| crate::win::file_icon_rgba(&src))
            .map(|(px, n)| {
                ctx.load_texture(
                    format!("icon:{command}"),
                    egui::ColorImage::from_rgba_unmultiplied([n, n], &px),
                    egui::TextureOptions::LINEAR,
                )
            });
        self.icons.insert(command.to_string(), tex.clone());
        tex
    }

    fn dirty(&self) -> bool {
        self.cfg != self.saved
    }

    fn managed(&self, r: &StartupRef) -> bool {
        self.cfg.apps.iter().any(|a| a.startup.as_ref() == Some(r))
    }

    fn add(&mut self, app: App) {
        self.cfg.apps.push(app);
        self.selected = Some(self.cfg.apps.len() - 1);
        self.view = View::Edit;
    }

    fn unique_name(&self, base: &str) -> String {
        let names: HashSet<&str> = self.cfg.apps.iter().map(|a| a.name.as_str()).collect();
        if !names.contains(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} {n}")).find(|n| !names.contains(n.as_str())).unwrap()
    }

    fn save(&mut self) {
        let mut seen = HashSet::new();
        for a in &self.cfg.apps {
            if a.name.trim().is_empty() {
                self.status = Some(("Every app needs a name.".into(), true));
                return;
            }
            if !seen.insert(a.name.clone()) {
                self.status = Some((format!("Two apps are named \"{}\".", a.name), true));
                return;
            }
        }
        if let Err(e) = config::save(&self.cfg) {
            self.status = Some((e, true));
            return;
        }
        let mut errs = Vec::new();
        // Entries no longer managed go back on, the way Windows had them.
        let now: Vec<_> = self.cfg.apps.iter().filter_map(|a| a.startup.clone()).collect();
        for r in self.saved.apps.iter().filter_map(|a| a.startup.as_ref()) {
            if !now.contains(r) {
                errs.extend(startup::set_enabled(r, true).err());
            }
        }
        if self.cfg.guard {
            errs.extend(startup::apply_guard(&self.cfg));
        }
        self.saved = self.cfg.clone();
        self.items = startup::scan();
        self.status = Some(if errs.is_empty() {
            ("Saved. Takes effect at next login.".into(), false)
        } else {
            (format!("Saved, but couldn't change: {}", errs.join("; ")), true)
        });
    }

    /// Estimated seconds after login, following `after` chains. CPU waits
    /// aren't counted (unknown), so those get a "+" in the label.
    fn estimate(&self, i: usize) -> f32 {
        suggest::estimate(&self.cfg, i)
    }

    // ---------- top bar ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("launchpad").heading().strong());
            ui.label(RichText::new("startup, in your order").weak());

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let dirty = self.dirty();
                if ui.add_enabled(dirty, primary_button("Save changes")).clicked() {
                    self.save();
                }
                if dirty && ui.button("Discard").clicked() {
                    self.cfg = self.saved.clone();
                    self.selected = self.selected.filter(|&i| i < self.cfg.apps.len());
                    self.status = None;
                }
                ui.add_space(8.0);
                ui.toggle_value(&mut self.show_log, "Last run");
                ui.toggle_value(&mut self.show_options, "Options");
                if let Some((msg, err)) = &self.status {
                    let c = if *err { DANGER } else { ui.visuals().weak_text_color() };
                    ui.label(RichText::new(msg).color(c));
                } else if dirty {
                    ui.label(RichText::new("Unsaved changes").color(WARN));
                }
            });
        });
        if !self.elevated {
            ui.add_space(2.0);
            banner(ui, WARN, "Opened without admin rights, so changes can't be saved. Open Settings from the tray icon instead.");
        }
        let n = self.suggestions.len();
        if n > 0 && self.view != View::Suggestions {
            ui.add_space(2.0);
            egui::Frame::default()
                .fill(ACCENT.gamma_multiply(0.15))
                .stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let what = if n == 1 { "1 suggestion".to_string() } else { format!("{n} suggestions") };
                        ui.label(RichText::new(format!("{what} from an assistant, waiting for you to review.")).color(ACCENT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Review").clicked() {
                                self.view = View::Suggestions;
                            }
                        });
                    });
                });
        }
        ui.add_space(6.0);
    }

    // ---------- left: start order ----------

    fn order_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Start order").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new("+ Add app").selected(self.view == View::Add)).clicked() {
                    self.view = View::Add;
                }
            });
        });
        ui.add_space(4.0);

        if self.cfg.apps.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No apps yet").strong());
                ui.label(RichText::new("Pick some from your startup items,\nor drop a .exe or shortcut here.").weak());
            });
            return;
        }

        let max_t = (0..self.cfg.apps.len()).map(|i| self.estimate(i)).fold(1.0f32, f32::max);
        let commands: Vec<String> = self.cfg.apps.iter().map(|a| a.command.clone()).collect();
        let icons: Vec<_> = commands.iter().map(|c| self.icon(ui.ctx(), c)).collect();
        let mut clicked = None;
        let mut toggled = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for i in 0..self.cfg.apps.len() {
                let a = &self.cfg.apps[i];
                let selected = self.selected == Some(i) && self.view == View::Edit;
                let t = self.estimate(i);
                let summary = summary(a);
                let (name, enabled, cpu) = (a.name.clone(), a.enabled, a.cpu_below.is_some());

                let fill = if selected {
                    ACCENT.gamma_multiply(0.18)
                } else {
                    ui.visuals().faint_bg_color
                };
                let stroke = if selected { Stroke::new(1.0_f32, ACCENT) } else { Stroke::NONE };
                let resp = egui::Frame::default()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut on = enabled;
                            if toggle(ui, &mut on).changed() {
                                toggled = Some(i);
                            }
                            app_icon(ui, icons[i].as_ref(), &name, 28.0, enabled);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                let n = RichText::new(if name.is_empty() { "(unnamed)" } else { &name }).strong();
                                ui.label(if enabled { n } else { n.weak() });
                                ui.label(RichText::new(summary).small().weak());
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if enabled {
                                    let label = format!("{}{}", fmt_secs(t), if cpu { "+" } else { "" });
                                    ui.label(RichText::new(label).small().monospace().color(ACCENT));
                                }
                            });
                        });
                        // Thin timeline bar: where this app lands relative to the last one.
                        if enabled {
                            let w = ui.available_width();
                            let (r, _) = ui.allocate_exact_size(Vec2::new(w, 3.0), Sense::hover());
                            ui.painter().rect_filled(r, 1.5, ui.visuals().widgets.noninteractive.bg_stroke.color);
                            let mut f = r;
                            f.set_width((w * (t / max_t)).max(3.0));
                            ui.painter().rect_filled(f, 1.5, ACCENT);
                        }
                    })
                    .response;
                if ui.interact(resp.rect, ui.id().with(("card", i)), Sense::click()).clicked() {
                    clicked = Some(i);
                }
                ui.add_space(2.0);
            }
        });

        if let Some(i) = toggled {
            self.cfg.apps[i].enabled ^= true;
        }
        if let Some(i) = clicked {
            self.selected = Some(i);
            self.view = View::Edit;
        }
    }

    // ---------- right: editor ----------

    fn editor(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.selected.filter(|&i| i < self.cfg.apps.len()) else {
            ui.centered_and_justified(|ui| ui.label(RichText::new("Select an app on the left.").weak()));
            return;
        };
        let others: Vec<String> = self
            .cfg
            .apps
            .iter()
            .enumerate()
            .filter(|(j, o)| *j != i && !o.name.is_empty())
            .map(|(_, o)| o.name.clone())
            .collect();
        let n = self.cfg.apps.len();
        let mut remove = false;
        let mut move_to = None;
        let mut rename = None;
        let mut unmanage = false;
        let cmd = self.cfg.apps[i].command.clone();
        let icon = self.icon(ui.ctx(), &cmd);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(12.0);
            let a = &mut self.cfg.apps[i];

            // Header: icon + name + actions
            ui.horizontal(|ui| {
                app_icon(ui, icon.as_ref(), &a.name, 36.0, true);
                let old = a.name.clone();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut a.name)
                        .font(egui::TextStyle::Heading)
                        .hint_text("App name")
                        .desired_width(300.0),
                );
                if r.changed() {
                    rename = Some((old, a.name.clone()));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Remove").color(DANGER))).clicked() {
                        remove = true;
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("Move down")).clicked() {
                        move_to = Some(i + 1);
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("Move up")).clicked() {
                        move_to = Some(i - 1);
                    }
                });
            });
            ui.horizontal(|ui| {
                toggle(ui, &mut a.enabled);
                ui.label(if a.enabled { "Starts at login" } else { "Off: launchpad won't start it" });
            });

            section(ui, "What to run");
            ui.add(
                egui::TextEdit::singleline(&mut a.command)
                    .hint_text(r#""C:\path\app.exe" --minimized"#)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
            hint(ui, "A program with arguments, a shortcut (.lnk), or shell:AppsFolder\\… for Store apps. %VARS% work.");

            section(ui, "When to start");
            ui.horizontal(|ui| {
                ui.label("Start after");
                let shown = a.after.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "login".into());
                egui::ComboBox::from_id_salt("after").selected_text(shown).width(170.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.after, None, "login");
                    for o in &others {
                        ui.selectable_value(&mut a.after, Some(o.clone()), o);
                    }
                });
                ui.label("then wait");
                ui.add(egui::DragValue::new(&mut a.delay).range(0.0..=900.0).speed(0.5).suffix(" s"));
            });
            ui.horizontal(|ui| {
                let mut on = a.cpu_below.is_some();
                if ui.checkbox(&mut on, "and wait until CPU use is below").changed() {
                    a.cpu_below = on.then_some(25);
                }
                let mut p = a.cpu_below.unwrap_or(25);
                if ui.add_enabled(on, egui::DragValue::new(&mut p).range(5..=95).suffix(" %")).changed() {
                    a.cpu_below = Some(p);
                }
            });
            hint(ui, "The CPU check needs 3 calm seconds in a row and gives up after 90 s.");

            section(ui, "Run as");
            ui.radio_value(&mut a.admin, false, "Me (recommended)");
            ui.radio_value(&mut a.admin, true, "Administrator");
            hint(ui, "Only pick Administrator for apps that need it, like Process Lasso or fan tools.");

            ui.add_space(8.0);
            egui::CollapsingHeader::new("Advanced").id_salt("adv").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Skip if this process is running:");
                    let mut proc = a.process.clone().unwrap_or_default();
                    let auto = crate::launch::process_name(&App { process: None, ..a.clone() }).unwrap_or_else(|| "none".into());
                    if ui.add(egui::TextEdit::singleline(&mut proc).hint_text(format!("auto: {auto}")).desired_width(200.0)).changed() {
                        a.process = (!proc.trim().is_empty()).then_some(proc);
                    }
                });
                match &a.startup {
                    Some(r) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Replaces Windows startup entry \"{}\" ({})", r.name, r.source.label()));
                            if ui.small_button("Stop managing").on_hover_text("Turns the Windows entry back on when you save.").clicked() {
                                unmanage = true;
                            }
                        });
                    }
                    None => hint(ui, "Not linked to a Windows startup entry."),
                }
            });
        });

        if let Some((old, new)) = rename {
            for a in &mut self.cfg.apps {
                if a.after.as_deref() == Some(old.as_str()) {
                    a.after = Some(new.clone());
                }
            }
        }
        if unmanage {
            self.cfg.apps[i].startup = None;
        }
        if let Some(j) = move_to {
            self.cfg.apps.swap(i, j);
            self.selected = Some(j);
        }
        if remove {
            let gone = self.cfg.apps.remove(i).name;
            for a in &mut self.cfg.apps {
                if a.after.as_deref() == Some(gone.as_str()) {
                    a.after = None;
                }
            }
            self.selected = if self.cfg.apps.is_empty() { None } else { Some(i.min(self.cfg.apps.len() - 1)) };
        }
    }

    // ---------- right: add view ----------

    fn add_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("Add an app").heading().strong());
        hint(ui, "Adding a startup item moves it into launchpad and turns its Windows entry off when you save.");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search startup items").desired_width(260.0));
            if ui.button("Rescan").clicked() {
                self.items = startup::scan();
            }
            if ui.button("Blank app").clicked() {
                let name = self.unique_name("New app");
                self.add(App { name, ..App::default() });
            }
        });
        ui.add_space(6.0);

        let commands: Vec<String> = self.items.iter().map(|it| it.command.clone()).collect();
        let mut icons: HashMap<String, Option<egui::TextureHandle>> = HashMap::new();
        for c in commands {
            let t = self.icon(ui.ctx(), &c);
            icons.insert(c, t);
        }
        let q = self.search.to_lowercase();
        let matches = |it: &&Item| q.is_empty() || it.r.name.to_lowercase().contains(&q) || it.command.to_lowercase().contains(&q);
        let (on, off): (Vec<&Item>, Vec<&Item>) =
            self.items.iter().filter(|it| !self.managed(&it.r)).filter(matches).partition(|it| it.enabled);
        let managed = self.items.iter().filter(|it| self.managed(&it.r)).count();

        let mut pick = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (title, list, note) in [
                ("Starting with Windows now", &on, "These slow your login today."),
                ("Already turned off", &off, "Add one to start it later instead of never."),
            ] {
                if list.is_empty() {
                    continue;
                }
                section(ui, &format!("{title} ({})", list.len()));
                hint(ui, note);
                for it in list {
                    let r = egui::Frame::default()
                        .fill(ui.visuals().faint_bg_color)
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let tex = icons.get(&it.command).cloned().flatten();
                                app_icon(ui, tex.as_ref(), &display_name(&it.r.name), 28.0, true);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.label(RichText::new(display_name(&it.r.name)).strong());
                                    ui.label(RichText::new(it.r.source.label()).small().weak());
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.button("Add").clicked() {
                                        pick = Some((*it).clone());
                                    }
                                });
                            });
                        })
                        .response;
                    r.on_hover_text(&it.command);
                }
            }
            if on.is_empty() && off.is_empty() {
                ui.add_space(20.0);
                ui.label(RichText::new(if q.is_empty() { "Nothing left to add." } else { "No matches." }).weak());
            }
            if managed > 0 {
                ui.add_space(8.0);
                hint(ui, &format!("{managed} startup item(s) already managed by launchpad aren't shown."));
            }
        });

        if let Some(it) = pick {
            let name = self.unique_name(&display_name(&it.r.name));
            self.add(App { name, command: it.command, startup: Some(it.r), ..App::default() });
        }
    }

    // ---------- extras ----------

    fn options_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_options;
        egui::Window::new("Options")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 56.0])
            .show(ctx, |ui| {
                ui.checkbox(&mut self.cfg.stay_resident, "Stay in the tray after launching");
                hint(ui, "Needed for the guard and the tray menu. Uses about 3 MB.");
                ui.add_enabled_ui(self.cfg.stay_resident, |ui| {
                    ui.checkbox(&mut self.cfg.guard, "Guard: keep managed entries off");
                });
                hint(ui, "Some apps turn their own startup entry back on after updating.\nThe guard switches it off again.");
                ui.add_space(4.0);
                ui.checkbox(&mut self.cfg.auto_update, "Install updates automatically");
                hint(ui, &format!("Checks GitHub every 12 hours. You're on v{}.", env!("CARGO_PKG_VERSION")));
            });
        self.show_options = open;
    }

    fn log_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("log").resizable(true).default_height(170.0).show_animated(ctx, self.show_log, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Last run").strong());
                if ui.small_button("Refresh").clicked() {
                    self.log = read_log();
                }
            });
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for line in self.log.lines() {
                    let c = if line.contains("FAILED") {
                        DANGER
                    } else if line.contains("skipped") || line.contains("gave up") || line.contains("never dropped") {
                        WARN
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.label(RichText::new(line).monospace().color(c));
                }
            });
        });
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        for f in files {
            let Some(p) = f.path else { continue };
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "App".into());
            let name = self.unique_name(&stem);
            self.add(App { name, command: format!("\"{}\"", p.display()), ..App::default() });
        }
    }
}

impl eframe::App for Ui {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.handle_drops(ctx);
        self.poll_suggestions(ctx);
        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui));
        self.log_panel(ctx);
        egui::SidePanel::left("order")
            .resizable(true)
            .default_width(360.0)
            .width_range(280.0..=520.0)
            .show(ctx, |ui| self.order_list(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::default().inner_margin(egui::Margin::symmetric(12, 0)).show(ui, |ui| match self.view {
                View::Edit => self.editor(ui),
                View::Add => self.add_view(ui),
                View::Suggestions => self.suggestions_view(ui),
            });
        });
        self.options_window(ctx);
    }
}

// ---------- small widgets ----------

/// App icon, or a colored circle with the first letter when there isn't one.
fn app_icon(ui: &mut egui::Ui, tex: Option<&egui::TextureHandle>, name: &str, size: f32, enabled: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let tint = if enabled { Color32::WHITE } else { Color32::from_white_alpha(90) };
    match tex {
        Some(t) => {
            egui::Image::new(t).tint(tint).paint_at(ui, rect);
        }
        None => {
            let h = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
            let hue = (h % 360) as f32 / 360.0;
            let c: Color32 = egui::ecolor::Hsva::new(hue, 0.55, 0.75, 1.0).into();
            let c = if enabled { c } else { c.gamma_multiply(0.35) };
            ui.painter().circle_filled(rect.center(), size / 2.0, c);
            let letter = name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(size * 0.5),
                Color32::WHITE,
            );
        }
    }
}

/// iOS-style switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(34.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool(resp.id, *on);
        let off_fill = ui.visuals().widgets.inactive.bg_fill;
        let fill = lerp_color(off_fill, ACCENT, t);
        let r = rect.height() / 2.0;
        ui.painter().rect_filled(rect, r, fill);
        let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), r - 3.0, Color32::WHITE);
    }
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE).strong()).fill(ACCENT)
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title.to_uppercase()).small().strong().color(ui.visuals().weak_text_color()));
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().weak());
}

fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::default()
        .fill(color.gamma_multiply(0.15))
        .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(color));
        });
}

/// One-line description of an app's rules for the list.
fn summary(a: &App) -> String {
    let mut parts = Vec::new();
    match a.after.as_deref().filter(|s| !s.is_empty()) {
        Some(dep) if a.delay > 0.0 => parts.push(format!("{} after {dep}", fmt_secs(a.delay))),
        Some(dep) => parts.push(format!("right after {dep}")),
        None if a.delay > 0.0 => parts.push(format!("{} after login", fmt_secs(a.delay))),
        None => parts.push("at login".into()),
    }
    if let Some(p) = a.cpu_below {
        parts.push(format!("CPU < {p}%"));
    }
    if a.admin {
        parts.push("admin".into());
    }
    if a.command.trim().is_empty() {
        parts.push("no command".into());
    }
    parts.join(" · ")
}

fn fmt_secs(s: f32) -> String {
    if s >= 60.0 {
        format!("{}m{:02}s", (s / 60.0) as u32, (s % 60.0) as u32)
    } else if s.fract() == 0.0 {
        format!("{}s", s as u32)
    } else {
        format!("{s:.1}s")
    }
}

/// Startup entry names are often reverse-DNS or file names; tidy them up.
fn display_name(raw: &str) -> String {
    let s = raw.trim_end_matches(".lnk").trim_end_matches(".url");
    let s = s.rsplit('.').next().filter(|_| s.starts_with("com.") || s.starts_with("electron.")).unwrap_or(s);
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => raw.to_string(),
    }
}

fn read_log() -> String {
    std::fs::read_to_string(config::log_path()).unwrap_or_else(|_| "(no run yet)".into())
}
