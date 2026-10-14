/// The desktop front end: an egui window over the same collector the text report uses.
///
/// Nothing in here computes a figure. Every number is read from a [`Snapshot`], so the window
/// and `morrow-watch --text` cannot disagree about what the machine is doing; this module only
/// decides where things go and what colour they are. That is also why there is no test module
/// here: the interesting logic is in the collector, the report and the CLI, all of which are
/// tested without a display.
///
/// The window is built with the same restraint as the rest of Morrow: one tick per second, no
/// animation, no gradient, and the process table only walked while it is on screen.
use std::process::ExitCode;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Stroke, Vec2};

use morrow_watch::cli::Options;
use morrow_watch::collector::{Collector, ProcessSample, ProcessSort, Snapshot, TickOptions};
use morrow_watch::util;

/// The palette. The same colours are used by the ANSI report, so a screenshot and a log line
/// describe the machine in one visual language.
const BACKGROUND: Color32 = Color32::from_rgb(18, 22, 25);
const SURFACE: Color32 = Color32::from_rgb(27, 33, 36);
const BORDER: Color32 = Color32::from_rgb(49, 58, 60);
const TEXT: Color32 = Color32::from_rgb(232, 236, 230);
const MUTED: Color32 = Color32::from_rgb(151, 163, 158);
const MINT: Color32 = Color32::from_rgb(133, 218, 177);
const AMBER: Color32 = Color32::from_rgb(240, 177, 103);
const ALERT: Color32 = Color32::from_rgb(224, 108, 117);

/// Chart height in points. The trend is a hint next to the figure, not the subject.
const CHART_HEIGHT: f32 = 46.0;

/// Which page is on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Overview,
    Processes,
}

/// The application state: one sampler, the last reading, and what the user is looking at.
pub struct WatchApp {
    collector: Collector,
    snapshot: Snapshot,
    view: View,
    /// Carries the `--filter` the program was started with, then the user's own typing.
    filter: String,
    sort: ProcessSort,
    /// The `--rows` budget, honoured exactly as the text report honours it.
    rows: usize,
    last_tick: Instant,
}

impl WatchApp {
    /// Open the window, already holding a first reading.
    ///
    /// The first tick happens here so that the window opens with figures on it rather than
    /// with zeros, and so that the counters sysinfo needs for a second reading exist before
    /// the first refresh is drawn.
    pub fn new(context: &eframe::CreationContext<'_>, options: Options) -> Self {
        context.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut collector = Collector::new();
        let snapshot = collector.tick(TickOptions::overview());
        Self {
            collector,
            snapshot,
            view: View::Overview,
            filter: options.filter.clone(),
            sort: options.sort,
            rows: options.rows,
            last_tick: Instant::now(),
        }
    }

    /// What the current page needs sampled.
    ///
    /// The overview does not draw the process table, and walking every process is the most
    /// expensive part of a tick on a two-core machine, so it is skipped until it is on screen.
    fn tick_options(&self) -> TickOptions {
        match self.view {
            View::Overview => TickOptions::overview(),
            View::Processes => TickOptions::everything(),
        }
    }

    /// Take a reading, and mark the moment it was taken.
    fn refresh(&mut self) {
        self.snapshot = self.collector.tick(self.tick_options());
        self.last_tick = Instant::now();
    }
}

/// A titled panel: the frame every section is drawn in.
///
/// `body` is a closure rather than a slice of pre-rendered widgets because egui lays out as it
/// goes: the panel can only be sized once its contents have been measured, so the contents have
/// to be built inside the frame.
fn card<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(8)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.label(RichText::new(title).size(10.0).strong().color(MUTED));
            ui.add_space(8.0);
            body(ui)
        })
        .inner
}

/// A page switch button. Returns true when it was clicked.
fn nav_button(ui: &mut egui::Ui, active: bool, label: &str) -> bool {
    let text = RichText::new(label)
        .size(12.0)
        .color(if active { TEXT } else { MUTED });
    let button = egui::Button::new(text).fill(if active { SURFACE } else { BACKGROUND });
    ui.add(button).clicked()
}

/// One labelled row with a meter: the GUI form of a single report line.
fn meter_row(ui: &mut egui::Ui, label: &str, detail: &str, percent: f32, accent: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [130.0, 18.0],
            egui::Label::new(RichText::new(label).size(12.0).color(TEXT)).truncate(),
        );
        // The figure is right-aligned and sized first, so the meters across rows line up even
        // though their captions do not.
        let detail = RichText::new(detail).size(11.0).color(MUTED);
        let detail_width = ui.painter().layout_no_wrap(
            detail.text().to_string(),
            FontId::proportional(11.0),
            MUTED,
        );
        let figure_width = detail_width.size().x + 12.0;
        let bar_width = (ui.available_width() - figure_width).max(40.0);
        ui.add_sized(
            [bar_width, 12.0],
            egui::ProgressBar::new(util::clamp_percent(percent) / 100.0)
                .fill(accent)
                .text(""),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(detail);
        });
    });
}

/// The line shown when a section has nothing to report.
///
/// An empty panel reads as a broken panel, so absence is stated rather than drawn blank.
fn nothing_to_show(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED));
}

/// A headline figure with a meter and a trend: the same shape as the report's PROCESSOR and
/// MEMORY sections, because it is read the same way.
fn metric_card(
    ui: &mut egui::Ui,
    title: &str,
    value: &str,
    detail: &str,
    percent: f32,
    history: &[f32],
    accent: Color32,
) {
    card(ui, title, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(value).size(30.0).strong().color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(detail).size(11.0).color(MUTED));
            });
        });
        ui.add_space(8.0);
        ui.add(
            egui::ProgressBar::new(util::clamp_percent(percent) / 100.0)
                .fill(accent)
                .desired_height(10.0)
                .text(""),
        );
        ui.add_space(8.0);
        draw_history(ui, history, accent);
    });
}

/// The trend, drawn as a line.
///
/// A monitor that shows only the current instant cannot answer "was it like this a minute
/// ago?", which is exactly the question that tells a slow machine from a busy one.
fn draw_history(ui: &mut egui::Ui, history: &[f32], colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), CHART_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    for fraction in [0.0, 0.5, 1.0] {
        let y = rect.bottom() - fraction * rect.height();
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0_f32, BORDER),
        );
    }
    if history.len() < 2 {
        return;
    }
    let last = (history.len() - 1) as f32;
    let points: Vec<egui::Pos2> = history
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let x = rect.left() + index as f32 / last * rect.width();
            let y = rect.bottom() - util::clamp_percent(*sample) / 100.0 * rect.height();
            egui::pos2(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2.0_f32, colour)));
}

/// The line under "PROCESSOR": what the chip is, and how the cores are loaded.
///
/// The per-core split is what answers "busy, or one core pinned?" on a two-core machine, so it
/// is shown whenever there is more than one core to split.
fn cpu_detail(snapshot: &Snapshot) -> String {
    let cpu = &snapshot.cpu;
    let mut parts: Vec<String> = Vec::new();
    let brand = cpu.brand.trim();
    if !brand.is_empty() {
        parts.push(brand.to_string());
    }
    if cpu.frequency_mhz > 0 {
        parts.push(format!("{} MHz", cpu.frequency_mhz));
    }
    if cpu.cores() > 1 {
        let per_core: Vec<String> = cpu
            .per_core
            .iter()
            .map(|core| util::percent(*core))
            .collect();
        parts.push(per_core.join("  "));
    }
    parts.join("  ·  ")
}

/// A small labelled figure, for the strip of facts above the storage and network cards.
fn info_block(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(10.0).strong().color(MUTED));
        ui.add_space(4.0);
        ui.label(
            RichText::new(value)
                .font(FontId::proportional(13.0))
                .color(TEXT),
        );
    });
}

/// Storage, one row per filesystem a person would recognise.
///
/// `snapshot.real_disks()` does the filtering, so the panel cannot disagree with the text
/// report about what counts as a disk.
fn storage_card(ui: &mut egui::Ui, snapshot: &Snapshot) {
    card(ui, "STORAGE", |ui| {
        let disks = snapshot.real_disks();
        if disks.is_empty() {
            nothing_to_show(ui, "no filesystem large enough to be storage");
            return;
        }
        for disk in disks {
            let percent = disk.percent_used();
            let mount = if disk.mount_point.is_empty() {
                "?"
            } else {
                disk.mount_point.as_str()
            };
            meter_row(
                ui,
                &format!("{}  {}", util::ellipsize(mount, 13), disk.file_system),
                &format!(
                    "{} of {}  ·  {} free",
                    util::bytes(disk.used()),
                    util::bytes(disk.total),
                    util::bytes(disk.available)
                ),
                percent,
                load_colour(percent, 75.0),
            );
        }
    });
}

/// Network interfaces, with idle links drawn quietly so the busy one stands out.
fn network_card(ui: &mut egui::Ui, snapshot: &Snapshot) {
    card(ui, "NETWORK", |ui| {
        let networks = snapshot.active_networks();
        if networks.is_empty() {
            nothing_to_show(ui, "no active interface");
            return;
        }
        for network in networks {
            let busy = network.bytes_per_second() > 0.0;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [110.0, 18.0],
                    egui::Label::new(
                        RichText::new(util::ellipsize(&network.name, 12))
                            .size(12.0)
                            .color(TEXT),
                    )
                    .truncate(),
                );
                ui.label(
                    RichText::new(format!(
                        "↓ {}   ↑ {}",
                        util::rate(network.received_per_second),
                        util::rate(network.transmitted_per_second)
                    ))
                    .size(12.0)
                    .color(if busy { MINT } else { MUTED }),
                );
            });
            ui.add_space(2.0);
        }
    });
}

/// Thermal zones, each drawn against its own ceiling: a 45 °C disk and a 45 °C processor are
/// not equally worrying, so they must not share one scale.
fn thermal_card(ui: &mut egui::Ui, snapshot: &Snapshot) {
    card(ui, "THERMALS", |ui| {
        let thermals = snapshot.readable_thermals();
        if thermals.is_empty() {
            nothing_to_show(ui, "no readable sensor");
            return;
        }
        for thermal in thermals {
            let percent = util::clamp_percent(thermal.percent_of_max());
            let current = thermal.celsius.unwrap_or(0.0);
            meter_row(
                ui,
                &util::ellipsize(&thermal.label, 14),
                &format!("{current:.1} °C   {}", util::percent(percent)),
                percent,
                load_colour(percent, 70.0),
            );
        }
    });
}

/// The colour a meter should be drawn in once it passes `warn`, and again past 90%.
fn load_colour(percent: f32, warn: f32) -> Color32 {
    if percent >= 90.0 {
        ALERT
    } else if percent >= warn {
        AMBER
    } else {
        MINT
    }
}

impl eframe::App for WatchApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let interval = Collector::sample_interval();
        let elapsed = self.last_tick.elapsed();
        if elapsed >= interval {
            self.refresh();
        } else {
            // Wake when the next reading is due instead of spinning at the display's refresh
            // rate: an idle monitor window should cost almost nothing on a 2-core machine.
            context.request_repaint_after(interval - elapsed);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(20.0))
            .show(context, |ui| {
                self.header(ui);
                ui.add_space(10.0);
                match self.view {
                    View::Overview => self.overview(ui),
                    View::Processes => self.processes(ui),
                }
            });
    }
}

impl WatchApp {
    /// Title, page switch and the machine's name: drawn on every page.
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MORROW").size(15.0).strong().color(MINT));
            ui.label(RichText::new("/  SYSTEM WATCH").size(12.0).color(MUTED));
            ui.add_space(24.0);
            if nav_button(ui, self.view == View::Overview, "Overview") {
                self.view = View::Overview;
            }
            if nav_button(ui, self.view == View::Processes, "Processes") {
                self.view = View::Processes;
            }
            let host = &self.snapshot.host;
            let name = if host.host_name.is_empty() {
                "morrow"
            } else {
                host.host_name.as_str()
            };
            let pressure = self.snapshot.memory.is_under_pressure();
            let uptime = util::duration_long(host.uptime);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pressure {
                    ui.label(
                        RichText::new("memory pressure")
                            .size(11.0)
                            .strong()
                            .color(ALERT),
                    );
                }
                ui.label(
                    RichText::new(format!("{name}  ·  up {uptime}"))
                        .size(11.0)
                        .color(MUTED),
                );
                ui.label(RichText::new("●").size(10.0).color(MINT));
            });
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(12.0);
    }

    /// The overview: the two figures people open a monitor for, then the rest of the machine.
    fn overview(&self, ui: &mut egui::Ui) {
        let snapshot = &self.snapshot;
        ui.columns(2, |columns| {
            let cpu = &snapshot.cpu;
            metric_card(
                &mut columns[0],
                "PROCESSOR",
                &util::percent(cpu.usage),
                &cpu_detail(snapshot),
                cpu.usage,
                &snapshot.cpu_history,
                MINT,
            );
            let memory = &snapshot.memory;
            metric_card(
                &mut columns[1],
                "MEMORY",
                &util::percent(memory.percent_used()),
                &format!(
                    "{} of {}  ·  {} free",
                    util::bytes(memory.used),
                    util::bytes(memory.total),
                    util::bytes(memory.available)
                ),
                memory.percent_used(),
                &snapshot.memory_history,
                if memory.is_under_pressure() {
                    ALERT
                } else {
                    AMBER
                },
            );
        });
        ui.add_space(12.0);

        let cores = snapshot.host.logical_cores.max(1);
        card(ui, "SYSTEM", |ui| {
            let load = &snapshot.load;
            ui.horizontal(|ui| {
                info_block(
                    ui,
                    "LOAD (1 MIN)",
                    &format!("{:.2}  ({:.0}%)", load.one, load.relative_to(cores)),
                );
                ui.add_space(34.0);
                info_block(ui, "KERNEL", &snapshot.host.kernel);
                ui.add_space(34.0);
                info_block(ui, "CORES", &format!("{cores} logical"));
            });
            if snapshot.memory.swap_total > 0 {
                ui.add_space(12.0);
                let swap = snapshot.memory.swap_percent_used();
                meter_row(
                    ui,
                    "swap",
                    &format!(
                        "{} of {} used",
                        util::bytes(snapshot.memory.swap_used),
                        util::bytes(snapshot.memory.swap_total)
                    ),
                    swap,
                    if swap >= 50.0 { ALERT } else { AMBER },
                );
            }
        });
        ui.add_space(12.0);
        storage_card(ui, snapshot);
        ui.add_space(12.0);
        ui.columns(2, |columns| {
            network_card(&mut columns[0], snapshot);
            thermal_card(&mut columns[1], snapshot);
        });
    }

    /// The process table: the page that answers "what is eating the RAM?".
    ///
    /// It is the only page that samples per-process data, and the sampler is told so on every
    /// tick: the overview does not pay for a table it does not draw.
    fn processes(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("filter").size(11.0).color(MUTED));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("name, user or pid")
                    .desired_width(160.0),
            );
            ui.add_space(18.0);
            ui.label(RichText::new("sort").size(11.0).color(MUTED));
            for (sort, label) in [
                (ProcessSort::Memory, "memory"),
                (ProcessSort::Cpu, "cpu"),
                (ProcessSort::Name, "name"),
            ] {
                if nav_button(ui, self.sort == sort, label) {
                    self.sort = sort;
                }
            }
        });
        ui.add_space(10.0);

        // A row budget of zero means "all of them", so it becomes a number here rather than a
        // special case inside the loop.
        let budget = if self.rows == 0 {
            usize::MAX
        } else {
            self.rows
        };
        let matching = self.snapshot.processes_matching(&self.filter, self.sort);
        let shown: Vec<&ProcessSample> = matching.iter().copied().take(budget).collect();
        let needle = self.filter.trim();
        let title = format!(
            "PROCESSES  ·  sorted by {}  ·  filter: {}  ·  showing {} of {}",
            sort_word(self.sort),
            if needle.is_empty() {
                "none".to_string()
            } else {
                util::ellipsize(needle, 24)
            },
            shown.len(),
            matching.len()
        );

        card(ui, &title, |ui| {
            if shown.is_empty() {
                nothing_to_show(ui, "nothing matched");
                return;
            }
            egui::Grid::new("morrow-processes")
                .num_columns(7)
                .spacing([14.0, 5.0])
                .striped(true)
                .show(ui, |ui| {
                    for heading in ["PID", "USER", "CPU%", "MEM%", "TIME", "STATE", "NAME"] {
                        ui.label(RichText::new(heading).size(10.0).strong().color(MUTED));
                    }
                    ui.end_row();
                    for process in &shown {
                        process_row(ui, &self.snapshot, process);
                    }
                });
        });
    }
}

/// One process row, in the same column order as the text report so the two can be read
/// side by side without re-learning where to look.
fn process_row(ui: &mut egui::Ui, snapshot: &Snapshot, process: &ProcessSample) {
    let memory = snapshot.process_memory_percent(process);
    ui.label(
        RichText::new(process.pid.to_string())
            .size(12.0)
            .color(MUTED),
    );
    ui.label(
        RichText::new(process.user.as_deref().unwrap_or("?"))
            .size(12.0)
            .color(MUTED),
    );
    ui.label(
        RichText::new(cpu_figure(process.cpu))
            .size(12.0)
            .color(if process.cpu >= 50.0 { AMBER } else { TEXT }),
    );
    ui.label(RichText::new(util::percent(memory)).size(12.0).color(TEXT));
    ui.label(
        RichText::new(util::duration(process.run_time))
            .size(12.0)
            .color(MUTED),
    );
    ui.label(RichText::new(&process.status).size(12.0).color(MUTED));
    ui.label(RichText::new(&process.name).size(12.0).color(TEXT));
    ui.end_row();
}

/// The word used for each ordering, matching the report's header.
fn sort_word(sort: ProcessSort) -> &'static str {
    match sort {
        ProcessSort::Memory => "memory",
        ProcessSort::Cpu => "cpu",
        ProcessSort::Name => "name",
    }
}

/// A process's CPU share, deliberately *not* clamped to 100%.
///
/// sysinfo measures against a single core, as `top` does, so a process saturating both cores of
/// a 2-core machine reads as 200%. Clamping that would describe the machine honestly and the
/// process deceptively.
fn cpu_figure(percent: f32) -> String {
    let value = if percent.is_finite() {
        percent.max(0.0)
    } else {
        0.0
    };
    format!("{value:.1}%")
}

/// Open the window and hand over to egui, returning the exit code the process should use.
///
/// The size lives here rather than in the app so that a monitor opened on a small screen starts
/// small: 940x660 fits inside the 1024x768 a Morrow desktop assumes, with room to spare.
pub fn run(options: Options) -> ExitCode {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Morrow Watch")
            .with_inner_size([940.0, 660.0])
            .with_min_inner_size([680.0, 480.0]),
        ..Default::default()
    };
    let created = eframe::run_native(
        "Morrow Watch",
        native_options,
        Box::new(move |context| Ok(Box::new(WatchApp::new(context, options)))),
    );
    match created {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // No DISPLAY, no GL driver, or a compositor that refused the context. Say so,
            // because the text front end still works and the person is one flag away from it.
            eprintln!("morrow-watch: cannot open the window: {error}");
            eprintln!("morrow-watch: use `--text` for a report in this terminal");
            ExitCode::FAILURE
        }
    }
}
