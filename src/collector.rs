//! Sampling layer: the only place in Morrow Watch that talks to `sysinfo`.
//!
//! Both front ends (the egui window and the text/JSON reporter) read from [`Collector`], so
//! the numbers they show cannot drift apart. The collector deliberately refreshes as little
//! as it can per tick: Morrow targets 2 cores and 500 MB, so the steady-state path never
//! reaches for `System::refresh_all()`, which walks every process, disk and thermal zone.
//!
//! Rates (bytes/s, per-second traffic) are derived here from absolute counters and the time
//! that actually elapsed between ticks. sysinfo also exposes "since the last refresh"
//! values, but those turn a slow tick into an apparent spike, and a monitor that is slow
//! *because* the machine is loaded is exactly when the numbers need to stay honest.

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use sysinfo::{
    Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind,
    ProcessStatus, ProcessesToUpdate, RefreshKind, System, UpdateKind, Users,
};

use crate::util;

/// Samples kept for the sparklines. At one sample per second this is a 90 second window,
/// which is as much history as stays legible in a ~40 column strip.
pub const HISTORY_LIMIT: usize = 90;

/// Below this, sysinfo refuses to report meaningful CPU usage: it measures load by taking
/// two readings of the kernel's per-CPU jiffy counters, so a tick that comes too soon after
/// the previous one divides by nearly nothing and produces noise.
pub const MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

/// `used / total` as a percentage, safe when `total` is zero.
///
/// Zero totals are real, not theoretical: containers and some virtual filesystems report a
/// zero-sized root, and dividing there yields a `NaN` that would then poison every bar,
/// sparkline and JSON field downstream.
pub fn percent_of(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used as f32 / total as f32 * 100.0
    }
}

/// Bytes per second between two readings of a monotonic kernel counter.
///
/// Counters legitimately go backwards: an interface is brought down and up between ticks, a
/// disk is remounted, or a suspended machine resumes with the counters it had. A reset must
/// read as "no traffic yet" rather than as a `u64`-sized spike that pins the graph for the
/// rest of the minute.
pub fn rate_between(previous: u64, current: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 || current < previous {
        return 0.0;
    }
    (current - previous) as f64 / seconds
}

/// Total machine load from sysinfo's per-core figures, on a 0..=100 scale.
///
/// Averaging the cores (rather than asking for a global figure) keeps the text and GUI
/// output identical to each other, and matches what `top` shows when told to summarise.
/// Non-finite cores are dropped rather than propagated: one chip returning `NaN` for a core
/// should not blank the whole machine.
pub fn average(per_core: &[f32]) -> f32 {
    let mut total = 0.0f32;
    let mut count = 0usize;
    for value in per_core {
        if value.is_finite() {
            total += *value;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        util::clamp_percent(total / count as f32)
    }
}

/// Physical memory, in bytes.
///
/// Binary units (KiB/MiB/GiB) are used throughout Morrow: the target ships busybox `free`
/// and `df`, which report in powers of two, and a monitor that disagrees with the console
/// next to it reads as broken.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemorySample {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

impl MemorySample {
    pub fn percent_used(&self) -> f32 {
        percent_of(self.used, self.total)
    }

    pub fn swap_percent_used(&self) -> f32 {
        percent_of(self.swap_used, self.swap_total)
    }

    /// True when the machine is close enough to the edge that Morrow should say so.
    pub fn is_under_pressure(&self) -> bool {
        self.percent_used() >= 90.0 || self.swap_percent_used() >= 50.0
    }
}

/// Machine-level load average.
///
/// On Linux these come straight from `/proc/loadavg`. Windows has no equivalent that
/// sysinfo can read, so the figures stay zero on a Windows build; nothing in the front ends
/// treats that as an error.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadSample {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

impl LoadSample {
    /// The 1-minute load as a percentage of the logical core count, where 100% means every
    /// core has exactly one runnable thread. This is the figure worth showing on a 2-core
    /// machine, where a raw "3.40" is hard to judge and "170%" is not.
    pub fn relative_to(&self, cores: usize) -> f32 {
        if cores == 0 {
            return 0.0;
        }
        util::clamp_percent((self.one / cores as f64 * 100.0) as f32)
    }
}

/// Processor load and identity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuSample {
    /// Average across all cores, 0..=100.
    pub usage: f32,
    /// Per-core load, 0..=100 each, in the order sysinfo reports them.
    pub per_core: Vec<f32>,
    pub brand: String,
    /// Current frequency of the first core, in MHz (0 when the platform will not say).
    pub frequency_mhz: u64,
}

impl CpuSample {
    pub fn cores(&self) -> usize {
        self.per_core.len()
    }
}

/// Filesystems smaller than this are kernel plumbing (`/sys/fs/cgroup`, `/dev/shm` on a
/// modest machine), not storage anyone put data on.
pub const MIN_VISIBLE_DISK: u64 = 4 * 1024 * 1024;

/// Filesystems that describe the kernel's own view of the world rather than any device.
///
/// `tmpfs` is deliberately absent: Morrow's whole subject is RAM, and a large tmpfs is a
/// real ramdisk holding real files. Only the tiny ones are filtered, by size.
pub const PSEUDO_FILE_SYSTEMS: [&str; 7] = [
    "sysfs", "proc", "devpts", "devtmpfs", "cgroup", "cgroup2", "debugfs",
];

/// One mount point, as `df` would describe it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskSample {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total: u64,
    /// Space free to unprivileged users, matching `df`'s Available column.
    pub available: u64,
    pub removable: bool,
}

impl DiskSample {
    pub fn used(&self) -> u64 {
        // Saturating rather than subtracting: a filesystem can report less total than it
        // reports available (overlayfs with reserved blocks), and a panic on the monitor is
        // the worst possible response to a full disk.
        self.total.saturating_sub(self.available)
    }

    pub fn percent_used(&self) -> f32 {
        percent_of(self.used(), self.total)
    }

    /// Whether this is storage a person would recognise.
    ///
    /// A stock Alpine mount table is mostly `sysfs`, `proc`, `cgroup2` and a scatter of
    /// 64 KiB ramdisks. Listing those as disks fills the panel with bars that are
    /// permanently at 100% and buries the one filesystem that matters.
    pub fn is_real_storage(&self) -> bool {
        self.total >= MIN_VISIBLE_DISK && !PSEUDO_FILE_SYSTEMS.contains(&self.file_system.as_str())
    }
}

/// One network interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkSample {
    pub name: String,
    pub received_per_second: f64,
    pub transmitted_per_second: f64,
    pub total_received: u64,
    pub total_transmitted: u64,
}

impl NetworkSample {
    pub fn total_bytes(&self) -> u64 {
        self.total_received.saturating_add(self.total_transmitted)
    }

    pub fn bytes_per_second(&self) -> f64 {
        self.received_per_second + self.transmitted_per_second
    }

    /// Loopback is real traffic but it is not *the network*, and on a machine serving a
    /// local compositor it is often the only interface with anything to report.
    pub fn is_loopback(&self) -> bool {
        let name = self.name.to_ascii_lowercase();
        name == "lo" || name.starts_with("lo@") || name.starts_with("loopback")
    }
}

/// One thermal zone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThermalSample {
    pub label: String,
    pub celsius: Option<f32>,
    pub max_celsius: Option<f32>,
    pub critical_celsius: Option<f32>,
}

impl ThermalSample {
    pub fn is_available(&self) -> bool {
        self.celsius.is_some_and(|value| value.is_finite())
    }

    /// Temperature as a percentage of the zone's own maximum, so a 45°C disk and a 45°C CPU
    /// can share one scale without pretending they are equally alarming.
    pub fn percent_of_max(&self) -> f32 {
        match (self.celsius, self.max_celsius) {
            (Some(current), Some(max)) if max.is_finite() && max > 0.0 => {
                util::clamp_percent(current / max * 100.0)
            }
            _ => 0.0,
        }
    }
}

/// One running process, reduced to what a monitor should show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessSample {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    /// sysinfo reports this as a percentage of a single core, so a busy two-thread process on
    /// a 2-core machine reads as 200%. Morrow keeps that convention: it is what `top` shows
    /// and what people compare against.
    pub cpu: f32,
    /// Resident memory, in bytes.
    pub memory: u64,
    pub run_time: u64,
    pub status: String,
    pub user: Option<String>,
}

impl ProcessSample {
    /// A stable identity for a process across refreshes.
    pub fn id(&self) -> u32 {
        self.pid
    }
}

/// How a process list should be ordered.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessSort {
    /// Biggest resident set first: the usual reason someone opens a monitor on a 500 MB box.
    #[default]
    Memory,
    /// Busiest core first, as `top` does by default.
    Cpu,
    /// Name first, for locating a specific daemon.
    Name,
}

/// The total order used by every process list in Morrow Watch.
///
/// Every comparison ends with the pid. That is not decoration: the list is rebuilt once a
/// second, and without a total order two processes with identical figures swap places
/// between frames, so the table appears to flicker and a row cannot be clicked reliably.
pub fn compare_processes(a: &ProcessSample, b: &ProcessSample, sort: ProcessSort) -> Ordering {
    match sort {
        ProcessSort::Memory => b
            .memory
            .cmp(&a.memory)
            .then_with(|| b.cpu.total_cmp(&a.cpu)),
        ProcessSort::Cpu => b
            .cpu
            .total_cmp(&a.cpu)
            .then_with(|| b.memory.cmp(&a.memory)),
        ProcessSort::Name => a
            .name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase()),
    }
    .then_with(|| a.pid.cmp(&b.pid))
}

/// Order a process list in place.
pub fn sort_processes(processes: &mut [ProcessSample], sort: ProcessSort) {
    processes.sort_by(|a, b| compare_processes(a, b, sort));
}

/// Case-insensitive search across the fields a person would actually type.
///
/// An empty or whitespace-only needle matches everything, so callers never need a special
/// case for "no filter". Matching the pid by string lets someone paste a pid from `ps`.
pub fn process_matches(process: &ProcessSample, needle: &str) -> bool {
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return true;
    }
    process.name.to_ascii_lowercase().contains(&needle)
        || process
            .user
            .as_ref()
            .is_some_and(|user| user.to_ascii_lowercase().contains(&needle))
        || process.pid.to_string() == needle
}

/// Every process matching `needle`, in the order given.
pub fn filter_processes<'a>(
    processes: &'a [ProcessSample],
    needle: &str,
) -> Vec<&'a ProcessSample> {
    processes
        .iter()
        .filter(|process| process_matches(process, needle))
        .collect()
}

/// A fixed-length window of percentages, oldest first, for sparklines and trends.
#[derive(Clone, Debug)]
pub struct History {
    samples: VecDeque<f32>,
    limit: usize,
}

impl History {
    /// A history that keeps at most `limit` samples.
    pub fn new(limit: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(limit.min(1024)),
            limit,
        }
    }

    /// The window Morrow's front ends actually use.
    pub fn default_window() -> Self {
        Self::new(HISTORY_LIMIT)
    }

    /// Record one sample, dropping the oldest once the window is full.
    ///
    /// Values are clamped on the way in: the sparkline scales to its own peak, so a single
    /// bad reading would otherwise flatten every honest sample around it for a whole minute.
    pub fn push(&mut self, value: f32) {
        self.samples.push_back(util::clamp_percent(value));
        while self.samples.len() > self.limit {
            self.samples.pop_front();
        }
    }

    /// The most recent sample, or `0.0` before anything has been recorded.
    pub fn latest(&self) -> f32 {
        self.samples.back().copied().unwrap_or(0.0)
    }

    /// The last `width` samples, oldest first, as a contiguous slice for rendering.
    pub fn tail(&self, width: usize) -> Vec<f32> {
        let take = width.min(self.samples.len());
        self.samples
            .iter()
            .skip(self.samples.len() - take)
            .copied()
            .collect()
    }

    /// The whole window, oldest first. Used by the JSON report.
    pub fn to_vec(&self) -> Vec<f32> {
        self.samples.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The window size this history was built with.
    pub fn limit(&self) -> usize {
        self.limit
    }
}

/// The sampler.
///
/// One per program, reused for its whole life: sysinfo keeps the previous CPU counters inside
/// `System`, so a collector rebuilt every second would have nothing to subtract from and
/// would report 0% busy forever.
pub struct Collector {
    system: System,
    disks: Disks,
    networks: Networks,
    components: Components,
    users: Users,
    cpu_history: History,
    memory_history: History,
    /// Last absolute byte counters per interface, so a rate can be divided by the time that
    /// actually elapsed rather than by the nominal tick length.
    network_previous: HashMap<String, (u64, u64)>,
    /// When traffic was last sampled, tracked separately from the main tick because the
    /// process view does not sample traffic at all.
    network_last_tick: Option<Instant>,
    last_tick: Option<Instant>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    /// A collector that has already read the cheap, unchanging facts about the machine.
    pub fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            system,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            users: Users::new_with_refreshed_list(),
            cpu_history: History::default_window(),
            memory_history: History::default_window(),
            network_previous: HashMap::new(),
            network_last_tick: None,
            last_tick: None,
        }
    }

    /// Time since the last sampling instant, or zero before the first tick.
    pub fn elapsed(&self) -> Duration {
        self.last_tick
            .map(|instant| instant.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    /// The interval the front ends should aim for.
    ///
    /// One second is the slowest rate at which CPU and memory still read as live, and the
    /// sampled machine has two cores: a monitor that costs more than it measures is a
    /// contradiction, so this is a constant, not a tuning knob.
    pub fn sample_interval() -> Duration {
        Duration::from_secs(1)
    }

    /// The CPU window, for a front end that wants history without a full snapshot.
    pub fn cpu_history(&self) -> &History {
        &self.cpu_history
    }

    /// The memory window.
    pub fn memory_history(&self) -> &History {
        &self.memory_history
    }

    /// Take one reading of the machine.
    ///
    /// A [`Snapshot`] is returned by value rather than exposed as shared state so that a
    /// front end can keep drawing the reading it already holds while the next one is being
    /// taken, and so that nothing it does can ever touch the sampler's counters.
    pub fn tick(&mut self, options: TickOptions) -> Snapshot {
        self.system.refresh_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram().with_swap()),
        );
        if options.processes {
            // CPU and memory per process only. Reading every process's command line,
            // working directory and environment is what makes a `top`-style refresh
            // expensive, and Morrow never shows any of them.
            self.system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing()
                    .with_cpu()
                    .with_memory()
                    // `OnlyIfNotSet` rather than `Always`: an owner is read once and never
                    // changes for the life of a process, so re-reading `/etc/passwd` per
                    // process per tick would be pure waste on a 2-core machine.
                    .with_user(UpdateKind::OnlyIfNotSet),
            );
        }

        // The sampling instant. Recorded here rather than at the end of the tick because this
        // is the moment the kernel's counters were read: pacing a tick from here makes the
        // front ends' one-second interval a real second between readings, with the cost of
        // walking the process table landing inside that second instead of on top of it.
        self.last_tick = Some(Instant::now());

        let cpu = self.sample_cpu();
        let memory = self.sample_memory();
        let host = self.sample_host();
        let load = self.sample_load();

        let disks = if options.disks {
            self.sample_disks()
        } else {
            Vec::new()
        };
        let networks = if options.network {
            self.sample_networks()
        } else {
            Vec::new()
        };
        let thermals = if options.sensors {
            self.sample_thermals()
        } else {
            Vec::new()
        };
        let processes = if options.processes {
            self.sample_processes()
        } else {
            Vec::new()
        };

        Snapshot {
            host,
            cpu,
            memory,
            load,
            disks,
            networks,
            thermals,
            processes,
            cpu_history: self.cpu_history.tail(HISTORY_LIMIT),
            memory_history: self.memory_history.tail(HISTORY_LIMIT),
        }
    }

    /// CPU load, averaged over the cores and recorded in the history window.
    fn sample_cpu(&mut self) -> CpuSample {
        let per_core: Vec<f32> = self
            .system
            .cpus()
            .iter()
            .map(|cpu| cpu.cpu_usage())
            .collect();
        // Averaging the cores, rather than asking for a global figure, keeps the text report,
        // the JSON report and the GUI on exactly the same number.
        let usage = if per_core.is_empty() {
            util::clamp_percent(self.system.global_cpu_usage())
        } else {
            average(&per_core)
        };
        self.cpu_history.push(usage);

        let first = self.system.cpus().first();
        CpuSample {
            usage,
            per_core,
            brand: first
                .map(|cpu| cpu.brand().trim().to_string())
                .unwrap_or_default(),
            frequency_mhz: first.map(|cpu| cpu.frequency()).unwrap_or(0),
        }
    }

    /// Physical and swap memory, with the RAM percentage recorded in its own window.
    fn sample_memory(&mut self) -> MemorySample {
        let memory = MemorySample {
            total: self.system.total_memory(),
            used: self.system.used_memory(),
            available: self.system.available_memory(),
            swap_total: self.system.total_swap(),
            swap_used: self.system.used_swap(),
        };
        self.memory_history.push(memory.percent_used());
        memory
    }

    /// The labels that identify this machine in a report.
    ///
    /// Every field has a fallback: a container or a stripped-down image can genuinely have no
    /// host name and a kernel that will not say what it is, and a report should say so rather
    /// than refuse to print.
    fn sample_host(&self) -> HostSample {
        HostSample {
            host_name: System::host_name().unwrap_or_else(|| "morrow".to_string()),
            os: System::long_os_version()
                .or_else(System::os_version)
                .unwrap_or_else(|| "unknown".to_string()),
            kernel: System::kernel_version().unwrap_or_else(|| "unknown".to_string()),
            uptime: System::uptime(),
            physical_cores: self.system.physical_core_count(),
            logical_cores: self.system.cpus().len(),
        }
    }

    /// The load averages, which Linux fills in from `/proc/loadavg`.
    ///
    /// On a machine with no `/proc` these stay at zero, which reads as "idle" rather than as
    /// an error — the honest answer, since nothing else about the machine is knowable either.
    fn sample_load(&self) -> LoadSample {
        let load = System::load_average();
        LoadSample {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        }
    }
}

/// Machine identity and uptime: facts that either never change or change on a human scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostSample {
    pub host_name: String,
    pub os: String,
    pub kernel: String,
    pub uptime: u64,
    pub physical_cores: Option<usize>,
    pub logical_cores: usize,
}

/// Which parts of the machine to sample on a given tick.
///
/// Walking every process and every thermal zone costs real time on a two-core box, and the
/// overview page does not draw them, so each front end asks for only what it will show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickOptions {
    pub processes: bool,
    pub disks: bool,
    pub sensors: bool,
    pub network: bool,
}

impl TickOptions {
    /// CPU, memory and load only: the cheapest tick that still means something.
    pub const fn core() -> Self {
        Self {
            processes: false,
            disks: false,
            sensors: false,
            network: false,
        }
    }

    /// What the overview page draws.
    pub const fn overview() -> Self {
        Self {
            processes: false,
            disks: true,
            sensors: true,
            network: true,
        }
    }

    /// Everything, including the process table.
    pub const fn everything() -> Self {
        Self {
            processes: true,
            disks: true,
            sensors: true,
            network: true,
        }
    }
}

impl Default for TickOptions {
    fn default() -> Self {
        Self::everything()
    }
}

/// One complete reading of the machine.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub host: HostSample,
    pub cpu: CpuSample,
    pub memory: MemorySample,
    pub load: LoadSample,
    pub disks: Vec<DiskSample>,
    pub networks: Vec<NetworkSample>,
    pub thermals: Vec<ThermalSample>,
    pub processes: Vec<ProcessSample>,
    pub cpu_history: Vec<f32>,
    pub memory_history: Vec<f32>,
}

impl Snapshot {
    /// A process's share of physical memory: the column people look for when the machine is
    /// short of it, and the one that justifies a 500 MB budget.
    pub fn process_memory_percent(&self, process: &ProcessSample) -> f32 {
        percent_of(process.memory, self.memory.total)
    }

    /// Disks worth drawing, in mount point order so `/` leads and the list is stable.
    pub fn real_disks(&self) -> Vec<&DiskSample> {
        let mut disks: Vec<&DiskSample> = self
            .disks
            .iter()
            .filter(|disk| disk.is_real_storage())
            .collect();
        disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
        disks
    }

    /// Network interfaces excluding loopback, busiest first.
    pub fn active_networks(&self) -> Vec<&NetworkSample> {
        let mut networks: Vec<&NetworkSample> = self
            .networks
            .iter()
            .filter(|network| !network.is_loopback())
            .collect();
        networks.sort_by(|a, b| {
            b.total_bytes()
                .cmp(&a.total_bytes())
                .then_with(|| a.name.cmp(&b.name))
        });
        networks
    }

    /// The process list as a front end shows it: filtered, then totally ordered.
    pub fn processes_matching(&self, needle: &str, sort: ProcessSort) -> Vec<&ProcessSample> {
        let mut selected = filter_processes(&self.processes, needle);
        selected.sort_by(|a, b| compare_processes(a, b, sort));
        selected
    }

    /// Sensors that actually reported a temperature.
    pub fn readable_thermals(&self) -> Vec<&ThermalSample> {
        self.thermals
            .iter()
            .filter(|thermal| thermal.is_available())
            .collect()
    }
}

/// A short, fixed-width word for a process state.
///
/// sysinfo's own `Display` is fine for logs, but these strings sit in a narrow column that
/// must not change width between frames, so they are chosen here instead.
fn status_label(status: ProcessStatus) -> &'static str {
    match status {
        ProcessStatus::Run => "run",
        ProcessStatus::Sleep => "sleep",
        ProcessStatus::Idle => "idle",
        ProcessStatus::Stop => "stop",
        ProcessStatus::Zombie => "zombie",
        ProcessStatus::Tracing => "tracing",
        ProcessStatus::Dead => "dead",
        ProcessStatus::Wakekill => "wakekill",
        ProcessStatus::Waking => "waking",
        ProcessStatus::Parked => "parked",
        ProcessStatus::LockBlocked => "lock",
        ProcessStatus::UninterruptibleDiskSleep => "io",
        ProcessStatus::Unknown(_) => "unknown",
    }
}

/// The parts of the sampler that touch disk, network, sensors and processes.
///
/// Kept in its own `impl` block because these are the sampling calls that can be expensive or
/// can fail outright on an odd machine, and they are the ones a front end is allowed to skip.
impl Collector {
    /// Mount points, re-read from the mount table.
    fn sample_disks(&mut self) -> Vec<DiskSample> {
        self.disks.refresh(true);
        self.disks
            .list()
            .iter()
            .map(|disk| DiskSample {
                name: disk.name().to_string_lossy().into_owned(),
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                total: disk.total_space(),
                available: disk.available_space(),
                removable: disk.is_removable(),
            })
            .collect()
    }

    /// Interface counters, turned into rates against the time *this* sampler last ran.
    ///
    /// The network window keeps its own clock rather than reusing the tick's. The process
    /// view samples no traffic at all, so without a separate clock, returning to the overview
    /// after a minute spent elsewhere would divide one burst of traffic by that whole minute
    /// and quietly under-report it.
    fn sample_networks(&mut self) -> Vec<NetworkSample> {
        self.networks.refresh(true);
        let elapsed = self
            .network_last_tick
            .map(|instant| instant.elapsed())
            .unwrap_or(Duration::ZERO);
        self.network_last_tick = Some(Instant::now());

        let mut samples = Vec::new();
        for (name, data) in self.networks.list().iter() {
            let received = data.total_received();
            let transmitted = data.total_transmitted();
            let (received_per_second, transmitted_per_second) =
                match self.network_previous.get(name) {
                    Some((previous_received, previous_transmitted)) => (
                        rate_between(*previous_received, received, elapsed),
                        rate_between(*previous_transmitted, transmitted, elapsed),
                    ),
                    // First sighting of an interface: there is no baseline yet, and inventing
                    // one would print a spike that never happened.
                    None => (0.0, 0.0),
                };
            self.network_previous
                .insert(name.clone(), (received, transmitted));
            samples.push(NetworkSample {
                name: name.clone(),
                received_per_second,
                transmitted_per_second,
                total_received: received,
                total_transmitted: transmitted,
            });
        }

        // Busiest interface first, so the overview shows the one that matters. Ties fall back
        // to the name to keep the order stable from frame to frame.
        samples.sort_by(|a, b| {
            b.total_bytes()
                .cmp(&a.total_bytes())
                .then_with(|| a.name.cmp(&b.name))
        });
        samples
    }

    /// Thermal zones. Most machines report none, which is not an error.
    fn sample_thermals(&mut self) -> Vec<ThermalSample> {
        self.components.refresh(true);
        self.components
            .list()
            .iter()
            .map(|component| ThermalSample {
                label: component.label().to_string(),
                celsius: component.temperature(),
                max_celsius: component.max(),
                critical_celsius: component.critical(),
            })
            .collect()
    }

    /// Every process, with user names resolved through the users list.
    ///
    /// A process whose owner cannot be resolved keeps `None` and is printed as `?`, which is
    /// the right answer for a kernel thread and for a system where `/etc/passwd` is absent.
    fn sample_processes(&self) -> Vec<ProcessSample> {
        self.system
            .processes()
            .iter()
            .map(|(pid, process)| ProcessSample {
                pid: pid.as_u32(),
                parent: process.parent().map(|parent| parent.as_u32()),
                name: process.name().to_string_lossy().into_owned(),
                cpu: process.cpu_usage(),
                memory: process.memory(),
                run_time: process.run_time(),
                status: status_label(process.status()).to_string(),
                user: process
                    .user_id()
                    .and_then(|uid| self.users.get_user_by_id(uid))
                    .map(|user| user.name().to_string()),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_of_never_divides_by_zero() {
        assert_eq!(percent_of(0, 0), 0.0);
        assert_eq!(percent_of(5, 0), 0.0);
        assert_eq!(percent_of(0, 100), 0.0);
        assert_eq!(percent_of(50, 100), 50.0);
        assert_eq!(percent_of(100, 100), 100.0);
        // Over-reporting totals are passed through: it is the renderer's job to clamp bars,
        // not this function's job to hide a filesystem that claims more used than it has.
        assert_eq!(percent_of(200, 100), 200.0);
    }

    #[test]
    fn rate_between_divides_by_elapsed_time() {
        assert_eq!(rate_between(0, 1000, Duration::from_secs(1)), 1000.0);
        assert_eq!(rate_between(0, 1000, Duration::from_secs(2)), 500.0);
        assert_eq!(rate_between(1000, 1000, Duration::from_secs(1)), 0.0);
        assert_eq!(rate_between(0, 1500, Duration::from_millis(500)), 3000.0);
    }

    #[test]
    fn rate_between_treats_counter_resets_as_idle() {
        // Interface came back up between ticks: current < previous must not underflow.
        assert_eq!(rate_between(5_000_000, 12, Duration::from_secs(1)), 0.0);
        assert_eq!(rate_between(0, 0, Duration::from_secs(0)), 0.0);
        // A zero-length interval is nonsense, not an infinite rate.
        assert_eq!(rate_between(10, 20, Duration::from_secs(0)), 0.0);
    }

    #[test]
    fn average_ignores_broken_cores() {
        assert_eq!(average(&[]), 0.0);
        assert_eq!(average(&[10.0]), 10.0);
        assert_eq!(average(&[0.0, 50.0, 100.0]), 50.0);
        // Two cores, one reporting NaN: the answer comes from the core that answered.
        assert_eq!(average(&[f32::NAN, 40.0]), 40.0);
        assert_eq!(average(&[f32::NAN, f32::INFINITY]), 0.0);
        // Clamped, so a bad driver cannot report 140% busy on a 0..=100 scale.
        assert_eq!(average(&[140.0, 140.0]), 100.0);
    }

    #[test]
    fn history_keeps_the_most_recent_samples() {
        let mut history = History::new(3);
        assert!(history.is_empty());
        assert_eq!(history.latest(), 0.0);

        for value in [1.0, 2.0, 3.0] {
            history.push(value);
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.to_vec(), vec![1.0, 2.0, 3.0]);
        assert_eq!(history.latest(), 3.0);

        history.push(4.0);
        assert_eq!(history.len(), 3);
        assert_eq!(history.to_vec(), vec![2.0, 3.0, 4.0]);
        assert_eq!(history.limit(), 3);
    }

    #[test]
    fn history_clamps_on_the_way_in() {
        let mut history = History::new(4);
        history.push(-20.0);
        history.push(f32::NAN);
        history.push(120.0);
        assert_eq!(history.to_vec(), vec![0.0, 0.0, 100.0]);
    }

    #[test]
    fn history_tail_returns_the_end_of_the_window() {
        let mut history = History::new(8);
        for value in [1.0, 2.0, 3.0, 4.0, 5.0] {
            history.push(value);
        }
        assert_eq!(history.tail(2), vec![4.0, 5.0]);
        assert_eq!(history.tail(5), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        // Asking for more than exists returns what there is, not a padded answer.
        assert_eq!(history.tail(99), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(history.tail(0), Vec::<f32>::new());
    }

    #[test]
    fn default_window_matches_the_documented_limit() {
        let history = History::default_window();
        assert_eq!(history.limit(), HISTORY_LIMIT);
        assert_eq!(HISTORY_LIMIT, 90);
    }

    /// Floating point tolerance: ratios such as 470/500 are not exact in binary, and a test
    /// that demanded bit-exactness would be testing IEEE 754 rather than Morrow.
    fn close(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() < 0.01
    }

    #[test]
    fn disk_usage_saturates_instead_of_underflowing() {
        // Overlayfs can report more available than total; that must not become a panic or a
        // nonsense eighteen-exabyte used figure.
        let contradictory = DiskSample {
            name: "mmcblk0p2".into(),
            mount_point: "/".into(),
            file_system: "ext4".into(),
            total: 1024,
            available: 4096,
            removable: false,
        };
        assert_eq!(contradictory.used(), 0);
        assert_eq!(contradictory.percent_used(), 0.0);

        let half_full = DiskSample {
            total: 200,
            available: 100,
            ..contradictory.clone()
        };
        assert_eq!(half_full.used(), 100);
        assert_eq!(half_full.percent_used(), 50.0);
    }

    #[test]
    fn pseudo_filesystems_are_not_reported_as_disks() {
        let sysfs = DiskSample {
            file_system: "sysfs".into(),
            mount_point: "/sys".into(),
            ..Default::default()
        };
        assert!(!sysfs.is_real_storage());

        let cgroup2 = DiskSample {
            file_system: "cgroup2".into(),
            mount_point: "/sys/fs/cgroup".into(),
            total: 64 * 1024 * 1024,
            ..Default::default()
        };
        assert!(!cgroup2.is_real_storage());

        // A big tmpfs has no device behind it, but it is a real ramdisk holding real files,
        // which is exactly what a RAM-focused monitor should be watching.
        let tmpfs = DiskSample {
            file_system: "tmpfs".into(),
            mount_point: "/tmp".into(),
            total: 256 * 1024 * 1024,
            available: 128 * 1024 * 1024,
            ..Default::default()
        };
        assert!(tmpfs.is_real_storage());

        // The 64 KiB ones are kernel plumbing.
        let shm = DiskSample {
            file_system: "tmpfs".into(),
            mount_point: "/dev/shm".into(),
            total: 65_536,
            available: 65_536,
            ..Default::default()
        };
        assert!(!shm.is_real_storage());

        let root = DiskSample {
            file_system: "ext4".into(),
            mount_point: "/".into(),
            total: 8 * 1024 * 1024 * 1024,
            available: 4 * 1024 * 1024 * 1024,
            ..Default::default()
        };
        assert!(root.is_real_storage());
    }

    #[test]
    fn network_totals_add_up_and_saturate() {
        let ethernet = NetworkSample {
            name: "eth0".into(),
            received_per_second: 1024.0,
            transmitted_per_second: 512.0,
            total_received: 4096,
            total_transmitted: 2048,
        };
        assert_eq!(ethernet.bytes_per_second(), 1536.0);
        assert_eq!(ethernet.total_bytes(), 6144);
        assert!(!ethernet.is_loopback());

        let wrapped = NetworkSample {
            total_received: u64::MAX,
            total_transmitted: u64::MAX,
            ..Default::default()
        };
        assert_eq!(wrapped.total_bytes(), u64::MAX);
    }

    #[test]
    fn loopback_is_recognised_by_name() {
        for name in ["lo", "LO", "lo@1", "Loopback Pseudo-Interface 1"] {
            let interface = NetworkSample {
                name: name.into(),
                ..Default::default()
            };
            assert!(interface.is_loopback(), "{name} should read as loopback");
        }
        for name in ["eth0", "wlan0", "enp0s3", "wlp2s0", "tun0"] {
            let interface = NetworkSample {
                name: name.into(),
                ..Default::default()
            };
            assert!(
                !interface.is_loopback(),
                "{name} should not read as loopback"
            );
        }
    }

    #[test]
    fn thermal_percentages_survive_missing_sensors() {
        let silent = ThermalSample::default();
        assert!(!silent.is_available());
        assert_eq!(silent.percent_of_max(), 0.0);

        let package = ThermalSample {
            label: "x86_pkg_temp".into(),
            celsius: Some(45.0),
            max_celsius: Some(90.0),
            critical_celsius: Some(100.0),
        };
        assert!(package.is_available());
        assert_eq!(package.percent_of_max(), 50.0);

        // A zone that never published a maximum has no scale to draw on.
        let maximum_unknown = ThermalSample {
            celsius: Some(70.0),
            ..Default::default()
        };
        assert_eq!(maximum_unknown.percent_of_max(), 0.0);

        let zero_max = ThermalSample {
            celsius: Some(70.0),
            max_celsius: Some(0.0),
            ..Default::default()
        };
        assert_eq!(zero_max.percent_of_max(), 0.0);

        let hot = ThermalSample {
            celsius: Some(120.0),
            max_celsius: Some(90.0),
            ..Default::default()
        };
        assert_eq!(hot.percent_of_max(), 100.0);
    }

    #[test]
    fn memory_pressure_follows_ram_and_swap() {
        let calm = MemorySample {
            total: 500 * 1024 * 1024,
            used: 200 * 1024 * 1024,
            available: 300 * 1024 * 1024,
            swap_total: 512 * 1024 * 1024,
            swap_used: 0,
        };
        assert!(close(calm.percent_used(), 40.0));
        assert!(!calm.is_under_pressure());

        let nearly_full = MemorySample {
            used: 470 * 1024 * 1024,
            available: 30 * 1024 * 1024,
            ..calm
        };
        assert!(close(nearly_full.percent_used(), 94.0));
        assert!(nearly_full.is_under_pressure());

        // RAM is comfortable but most of swap is gone: still worth flagging, because that is
        // the state a 500 MB machine reaches shortly before it stops responding.
        let swapping = MemorySample {
            total: 500 * 1024 * 1024,
            used: 200 * 1024 * 1024,
            available: 300 * 1024 * 1024,
            swap_total: 1024 * 1024 * 1024,
            swap_used: 600 * 1024 * 1024,
        };
        assert!(swapping.is_under_pressure());

        // No swap configured at all is not pressure.
        let no_swap = MemorySample {
            total: 100,
            used: 10,
            available: 90,
            swap_total: 0,
            swap_used: 0,
        };
        assert!(!no_swap.is_under_pressure());
    }

    #[test]
    fn load_is_scaled_by_core_count() {
        let load = LoadSample {
            one: 1.0,
            five: 0.8,
            fifteen: 0.5,
        };
        assert_eq!(load.relative_to(0), 0.0);
        assert!(close(load.relative_to(1), 100.0));
        assert!(close(load.relative_to(2), 50.0));
        assert!(close(load.relative_to(4), 25.0));

        let swamped = LoadSample {
            one: 9.0,
            five: 9.0,
            fifteen: 9.0,
        };
        assert_eq!(swamped.relative_to(2), 100.0);
    }

    #[test]
    fn sample_interval_stays_above_sysinfo_noise() {
        // Both front ends tick once per second; guard against anyone "optimising" that below
        // the point where sysinfo's CPU figures stop meaning anything.
        assert!(MIN_SAMPLE_INTERVAL >= Duration::from_millis(200));
        assert!(MIN_SAMPLE_INTERVAL < Duration::from_secs(1));
    }

    #[test]
    fn a_full_tick_reports_a_self_consistent_machine() {
        let mut collector = Collector::new();
        let first = collector.tick(TickOptions::everything());

        assert!(
            first.memory.total > 0,
            "a machine with no memory is not a machine"
        );
        assert!(first.memory.used <= first.memory.total);
        assert!((0.0..=100.0).contains(&first.cpu.usage));
        assert!(!first.cpu.per_core.is_empty());
        assert_eq!(first.cpu.cores(), first.host.logical_cores);
        assert_eq!(first.cpu_history.len(), 1);
        assert_eq!(collector.cpu_history().len(), 1);

        let waited = MIN_SAMPLE_INTERVAL + Duration::from_millis(100);
        std::thread::sleep(waited);
        let second = collector.tick(TickOptions::everything());

        assert_eq!(second.cpu_history.len(), 2);
        assert_eq!(collector.memory_history().len(), 2);
        assert!(second.host.uptime > 0);
        // A tick restarts the interval the front ends pace themselves against, so the wait
        // just slept through is behind us by the time the reading lands.
        assert!(
            collector.elapsed() < waited,
            "a tick should restart the interval rather than carry the wait behind it"
        );
    }

    #[test]
    fn a_core_only_tick_skips_the_expensive_parts() {
        let mut collector = Collector::new();
        let snapshot = collector.tick(TickOptions::core());

        // The cheap half still arrives...
        assert!(snapshot.memory.total > 0);
        assert!(!snapshot.cpu.per_core.is_empty());
        // ...and nothing that was not asked for is sampled at all.
        assert!(snapshot.disks.is_empty());
        assert!(snapshot.networks.is_empty());
        assert!(snapshot.thermals.is_empty());
        assert!(snapshot.processes.is_empty());

        // A waited-for interval is visible before the next tick...
        std::thread::sleep(Duration::from_millis(200));
        assert!(collector.elapsed() >= Duration::from_millis(150));
        // ...and ticking restarts it rather than accumulating a backlog of catch-up work.
        let later = collector.tick(TickOptions::core());
        assert!(collector.elapsed() < Duration::from_millis(150));
        assert_eq!(later.cpu_history.len(), 2);
    }
}
