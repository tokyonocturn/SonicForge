//! SonicForge editor.
//!
//! A live parametric EQ editor: the frequency response is computed by the same
//! engine the APO runs, so the curve on screen is the curve you hear. Bands can
//! be dragged directly on the graph, edited numerically, or replaced by one of
//! the built-in presets.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod plot;

use eframe::egui;
use sf_core::config::{builtin_presets, Document, Preset};
use sf_core::control::ControlBlock;
use sf_core::engine::{compile_document, CompileOptions, Stage};
use sf_win::endpoints::{self, Endpoint, Flow};
use std::path::PathBuf;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(94, 196, 255);
const GRID: egui::Color32 = egui::Color32::from_rgb(46, 52, 64);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(150, 158, 172);
const CURVE: egui::Color32 = egui::Color32::from_rgb(120, 220, 170);

fn main() -> eframe::Result<()> {
    sf_win::logging::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([860.0, 560.0])
            .with_title("SonicForge"),
        ..Default::default()
    };
    eframe::run_native(
        "SonicForge",
        options,
        Box::new(|cc| Ok(Box::new(Editor::new(cc)))),
    )
}

struct Editor {
    preset: Preset,
    presets: Vec<Preset>,
    freqs: Vec<f64>,
    response: Vec<f64>,
    selected: Option<usize>,
    dragging: Option<usize>,
    dirty: bool,
    diagnostics: Vec<String>,
    status: String,
    devices: Vec<Endpoint>,
    device_error: Option<String>,
    restrict_to_device: bool,
    selected_device: Option<usize>,
    bypass: bool,
    master_preamp: f32,
    control: Option<ControlBlock>,
    show_text: bool,
    text: String,
    config_path: PathBuf,
    latency: usize,
    stage_count: usize,
}

impl Editor {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let config_path = match sf_win::paths::ensure_config_layout() {
            Ok(path) => path,
            Err(e) => {
                sf_win::logging::error(&format!("cannot create configuration layout: {e}"));
                sf_win::paths::config_file()
            }
        };
        let text = std::fs::read_to_string(&config_path).unwrap_or_default();
        let preset = Preset::from_document(&Document::parse(&text));
        let devices = match endpoints::list(Flow::Render) {
            Ok(list) => {
                let mut error = None;
                if list.is_empty() {
                    error = Some("No active playback devices found.".to_string());
                }
                let _ = error;
                list
            }
            Err(e) => {
                let mut fallback = Vec::new();
                fallback.push(Endpoint {
                    id: String::new(),
                    name: format!("device enumeration failed: {e}"),
                    connection: String::new(),
                    flow: Flow::Render,
                    is_default: false,
                    guid: String::new(),
                });
                fallback
            }
        };
        let device_error = if devices.len() == 1 && devices[0].id.is_empty() {
            Some(devices[0].name.clone())
        } else {
            None
        };
        let control = ControlBlock::create().ok();
        let (bypass, master_preamp) = control
            .as_ref()
            .map(|c| {
                let snap = c.read();
                (snap.bypass, snap.preamp_db)
            })
            .unwrap_or((false, 0.0));

        let freqs: Vec<f64> = (0..=360)
            .map(|i| plot::unit_to_freq(i as f64 / 360.0))
            .collect();

        let mut editor = Self {
            preset,
            presets: builtin_presets(),
            freqs,
            response: Vec::new(),
            selected: None,
            dragging: None,
            dirty: true,
            diagnostics: Vec::new(),
            status: format!("Configuration: {}", config_path.display()),
            devices,
            device_error,
            restrict_to_device: false,
            selected_device: None,
            bypass,
            master_preamp,
            control,
            show_text: false,
            text,
            config_path,
            latency: 0,
            stage_count: 0,
        };
        editor.selected_device = editor.devices.iter().position(|d| d.is_default);
        editor.recompute();
        editor
    }

    /// Configuration text that would be written for the current state.
    fn compose_text(&self) -> String {
        let mut text = String::new();
        if self.restrict_to_device {
            if let Some(index) = self.selected_device {
                if let Some(device) = self.devices.get(index) {
                    text.push_str(&format!(
                        "# Applies only to this device\nDevice: {}\n",
                        device.name
                    ));
                }
            }
        }
        text.push_str(&self.preset.to_config_text());
        text
    }

    /// Recompiles the response curve and diagnostics.
    fn recompute(&mut self) {
        let text = self.compose_text();
        let doc = Document::parse(&text);
        let device = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .map(|d| d.device_info())
            .unwrap_or_default();
        let options = CompileOptions {
            sample_rate: 48000,
            channels: 2,
            device,
            stage: Stage::PostMix,
            ..Default::default()
        };
        let mut chain = compile_document(&doc, &options);
        self.response = chain.pipeline.frequency_response(&self.freqs);
        self.diagnostics = chain.diagnostics.iter().map(|d| d.to_string()).collect();
        self.latency = chain.pipeline.latency_samples();
        self.stage_count = chain.pipeline.ops().len();
        self.dirty = false;
    }

    fn apply(&mut self) {
        let text = if self.show_text {
            self.text.clone()
        } else {
            self.compose_text()
        };
        if self.config_path.exists() {
            let backup = self.config_path.with_extension("txt.bak");
            if let Err(e) = std::fs::copy(&self.config_path, &backup) {
                self.status = format!("Backup failed: {e}");
                return;
            }
        }
        match std::fs::write(&self.config_path, &text) {
            Ok(()) => {
                if let Some(control) = &self.control {
                    control.bump_config();
                }
                self.status = format!("Applied to {}", self.config_path.display());
                let doc = Document::parse(&text);
                self.preset = Preset::from_document(&doc);
                self.recompute();
            }
            Err(e) => self.status = format!("Cannot write configuration: {e}"),
        }
    }

    fn reload_from_disk(&mut self) {
        match std::fs::read_to_string(&self.config_path) {
            Ok(text) => {
                self.preset = Preset::from_document(&Document::parse(&text));
                self.text = text;
                self.selected = None;
                self.status = "Reloaded from disk".into();
                self.recompute();
            }
            Err(e) => self.status = format!("Cannot read configuration: {e}"),
        }
    }

    fn apply_preset(&mut self, index: usize) {
        if let Some(preset) = self.presets.get(index).cloned() {
            self.preset = preset;
            self.selected = None;
            self.recompute();
            self.status = format!(
                "Preset '{}' loaded (press Apply to write it)",
                self.preset.name
            );
        }
    }

    fn toggle_bypass(&mut self) {
        if let Some(control) = &self.control {
            let snap = control.set_bypass(self.bypass);
            self.bypass = snap.bypass;
            self.status = if self.bypass {
                "Bypassed: audio passes through untouched".into()
            } else {
                "Processing enabled".into()
            };
        } else {
            self.status = "Control block unavailable".into();
        }
    }

    fn draw_graph(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().max(320.0);
        let (response, painter) =
            ui.allocate_painter(egui::vec2(width, 320.0), egui::Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, 8.0, egui::Color32::from_rgb(16, 18, 24));

        let x_of = |freq: f64| rect.left() + plot::freq_to_unit(freq) as f32 * rect.width();
        let y_of = |db: f64| rect.top() + plot::db_to_unit(db) as f32 * rect.height();
        let db_of = |y: f32| plot::unit_to_db(((y - rect.top()) / rect.height()) as f64);

        // Grid and labels.
        for freq in plot::FREQ_TICKS {
            let x = x_of(freq);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, GRID),
            );
            painter.text(
                egui::pos2(x + 3.0, rect.bottom() - 2.0),
                egui::Align2::LEFT_BOTTOM,
                plot::freq_label(freq),
                egui::FontId::proportional(11.0),
                TEXT_DIM,
            );
        }
        for db in plot::DB_TICKS {
            let y = y_of(db);
            let stroke = if db == 0.0 {
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(78, 86, 102))
            } else {
                egui::Stroke::new(1.0_f32, GRID)
            };
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                stroke,
            );
            painter.text(
                egui::pos2(rect.left() + 4.0, y),
                egui::Align2::LEFT_TOP,
                plot::db_label(db),
                egui::FontId::proportional(11.0),
                TEXT_DIM,
            );
        }

        // Response curve.
        if self.response.len() == self.freqs.len() && !self.response.is_empty() {
            let points: Vec<egui::Pos2> = self
                .freqs
                .iter()
                .zip(self.response.iter())
                .map(|(f, db)| egui::pos2(x_of(*f), y_of(*db)))
                .collect();
            painter.add(egui::Shape::line(
                points.clone(),
                egui::Stroke::new(2.0_f32, CURVE),
            ));
            // Shade between the curve and 0 dB for readability.
            let baseline = y_of(0.0);
            let mut polygon = points.clone();
            polygon.push(egui::pos2(rect.right(), baseline));
            polygon.push(egui::pos2(rect.left(), baseline));
            painter.add(egui::Shape::convex_polygon(
                polygon,
                egui::Color32::from_rgba_unmultiplied(120, 220, 170, 22),
                egui::Stroke::NONE,
            ));
        }

        // Band handles.
        for (index, band) in self.preset.bands.iter().enumerate() {
            let pos = egui::pos2(x_of(band.freq), y_of(band.gain_db));
            let selected = self.selected == Some(index);
            let radius = if selected { 7.0 } else { 5.0 };
            let color = if band.enabled {
                if selected {
                    ACCENT
                } else {
                    CURVE
                }
            } else {
                TEXT_DIM
            };
            painter.circle_filled(pos, radius, color);
            if selected {
                painter.circle_stroke(pos, radius + 3.0, egui::Stroke::new(1.5_f32, ACCENT));
                painter.text(
                    egui::pos2(pos.x + 12.0, pos.y - 8.0),
                    egui::Align2::LEFT_TOP,
                    format!(
                        "{} Hz  {} dB",
                        band.freq.round(),
                        plot::db_label(band.gain_db)
                    ),
                    egui::FontId::proportional(12.0),
                    ACCENT,
                );
            }
        }

        // Interaction: pick the nearest handle, then drag it.
        let pointer = response.interact_pointer_pos();
        let nearest = pointer.and_then(|pos| {
            self.preset
                .bands
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let handle = egui::pos2(x_of(b.freq), y_of(b.gain_db));
                    (i, handle.distance(pos))
                })
                .filter(|(_, d)| *d < 16.0)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
        });

        if response.drag_started() {
            self.dragging = nearest;
            if let Some(index) = nearest {
                self.selected = Some(index);
            }
        }
        if response.dragged() {
            if let (Some(index), Some(pos)) = (self.dragging, pointer) {
                if let Some(band) = self.preset.bands.get_mut(index) {
                    let freq = plot::unit_to_freq(((pos.x - rect.left()) / rect.width()) as f64);
                    let gain = db_of(pos.y).clamp(-plot::DB_RANGE, plot::DB_RANGE);
                    band.freq = freq.clamp(20.0, 20000.0);
                    band.gain_db = (gain * 10.0).round() / 10.0;
                    self.dirty = true;
                }
            }
        }
        if response.drag_stopped() {
            self.dragging = None;
        }
        if response.double_clicked() {
            if let Some(pos) = pointer {
                let freq = plot::unit_to_freq(((pos.x - rect.left()) / rect.width()) as f64);
                let gain = db_of(pos.y).clamp(-plot::DB_RANGE, plot::DB_RANGE);
                self.preset.bands.push(sf_core::config::PresetBand::peak(
                    (freq * 10.0).round() / 10.0,
                    (gain * 10.0).round() / 10.0,
                    1.4,
                ));
                self.selected = Some(self.preset.bands.len() - 1);
                self.dirty = true;
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading("Devices");
        if let Some(error) = &self.device_error {
            ui.colored_label(egui::Color32::from_rgb(240, 160, 90), error);
        }
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .id_salt("devices")
            .show(ui, |ui| {
                for (index, device) in self.devices.iter().enumerate() {
                    if device.id.is_empty() {
                        continue;
                    }
                    let label = format!(
                        "{}{}",
                        if device.is_default { "* " } else { "" },
                        device.name
                    );
                    if ui
                        .selectable_label(self.selected_device == Some(index), label)
                        .clicked()
                    {
                        self.selected_device = Some(index);
                    }
                }
            });
        ui.checkbox(
            &mut self.restrict_to_device,
            "Apply only to the selected device",
        );

        ui.separator();
        ui.heading("Presets");
        for index in 0..self.presets.len() {
            let name = self.presets[index].name.clone();
            if ui.button(name).clicked() {
                self.apply_preset(index);
            }
        }

        ui.separator();
        ui.heading("Bands");
        let mut remove: Option<usize> = None;
        for index in 0..self.preset.bands.len() {
            let selected = self.selected == Some(index);
            egui::Frame::none()
                .fill(if selected {
                    egui::Color32::from_rgb(32, 40, 54)
                } else {
                    egui::Color32::TRANSPARENT
                })
                .inner_margin(egui::Margin::symmetric(4.0, 2.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let band = &mut self.preset.bands[index];
                        if ui.checkbox(&mut band.enabled, "").changed() {
                            self.dirty = true;
                        }
                        if ui
                            .add(
                                egui::DragValue::new(&mut band.freq)
                                    .range(20.0..=20000.0)
                                    .speed(2.0)
                                    .suffix(" Hz"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                            self.selected = Some(index);
                        }
                        if ui
                            .add(
                                egui::DragValue::new(&mut band.gain_db)
                                    .range(-24.0..=24.0)
                                    .speed(0.1)
                                    .suffix(" dB"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                            self.selected = Some(index);
                        }
                        if ui
                            .add(
                                egui::DragValue::new(&mut band.q)
                                    .range(0.1..=18.0)
                                    .speed(0.01)
                                    .prefix("Q "),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui.small_button("x").clicked() {
                            remove = Some(index);
                        }
                    });
                });
        }
        if let Some(index) = remove {
            self.preset.bands.remove(index);
            self.selected = None;
            self.dirty = true;
        }
        if ui.button("+ Add band").clicked() {
            self.preset
                .bands
                .push(sf_core::config::PresetBand::peak(1000.0, 0.0, 1.4));
            self.selected = Some(self.preset.bands.len() - 1);
            self.dirty = true;
        }
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.dirty {
            self.recompute();
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("SonicForge");
                ui.label(egui::RichText::new(format!("v{}", sf_core::VERSION)).color(TEXT_DIM));
                ui.separator();
                let mut bypass = self.bypass;
                if ui.checkbox(&mut bypass, "Bypass").changed() {
                    self.bypass = bypass;
                    self.toggle_bypass();
                }
                ui.separator();
                ui.label("Master");
                let mut preamp = self.master_preamp;
                if ui
                    .add(
                        egui::Slider::new(&mut preamp, -30.0..=12.0)
                            .suffix(" dB")
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    self.master_preamp = preamp;
                    if let Some(control) = &self.control {
                        control.set_preamp_db(preamp);
                    }
                }
                ui.separator();
                if ui.button("Apply").clicked() {
                    self.apply();
                }
                if ui.button("Reload").clicked() {
                    self.reload_from_disk();
                }
                ui.checkbox(&mut self.show_text, "Edit as text");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} stages · {} ms latency",
                            self.stage_count,
                            (self.latency as f64 / 48.0).round() as i64
                        ))
                        .color(TEXT_DIM),
                    );
                });
            });
        });

        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(330.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.status).color(TEXT_DIM));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.show_text {
                ui.horizontal(|ui| {
                    ui.label("Raw configuration (Equalizer APO syntax)");
                    if ui.button("Parse into bands").clicked() {
                        self.preset = Preset::from_document(&Document::parse(&self.text));
                        self.status = "Parsed text into bands".into();
                        self.dirty = true;
                    }
                });
                let mut text = std::mem::take(&mut self.text);
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(22)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
                self.text = text;
            } else {
                ui.horizontal(|ui| {
                    ui.label("Preamp");
                    let mut preamp = self.preset.preamp_db;
                    if ui
                        .add(
                            egui::Slider::new(&mut preamp, -30.0..=12.0)
                                .suffix(" dB")
                                .fixed_decimals(1),
                        )
                        .changed()
                    {
                        self.preset.preamp_db = preamp;
                        self.dirty = true;
                    }
                    if ui.button("Auto preamp").clicked() {
                        self.preset.preamp_db = self.preset.suggested_preamp();
                        self.dirty = true;
                    }
                    ui.label(
                        egui::RichText::new("Drag bands on the graph · double click to add")
                            .color(TEXT_DIM),
                    );
                });
                self.draw_graph(ui);
            }

            ui.separator();
            ui.heading("Diagnostics");
            if self.diagnostics.is_empty() {
                ui.label(egui::RichText::new("Configuration is valid.").color(CURVE));
            } else {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .id_salt("diagnostics")
                    .show(ui, |ui| {
                        for message in &self.diagnostics {
                            let color = if message.contains("error") {
                                egui::Color32::from_rgb(240, 120, 120)
                            } else {
                                egui::Color32::from_rgb(230, 190, 110)
                            };
                            ui.colored_label(color, message);
                        }
                    });
            }
        });
    }
}
