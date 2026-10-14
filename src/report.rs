//! The text and JSON front ends: pure formatting over a [`Snapshot`].
//!
//! The same binary runs on the Openbox desktop and in a rescue shell, so nothing in here
//! touches a terminal, a locale or the filesystem. [`text`] and [`json`] take a reading and
//! return a `String`, which keeps both outputs testable without an X11 server and keeps
//! `--json` safe to pipe into a script.
//!
//! Both outputs are written by hand rather than through a serialisation crate. The schema is
//! small and fixed, and a monitor that ships inside a 500 MB image should not grow a
//! dependency tree just to print it.

use crate::collector::{
    DiskSample, NetworkSample, ProcessSample, ProcessSort, Snapshot, ThermalSample,
};
use crate::util;

/// The section accents, chosen to match the colours the GUI uses for the same numbers.
const MINT: (u8, u8, u8) = (0x85, 0xDA, 0xB1);
const AMBER: (u8, u8, u8) = (0xF0, 0xB1, 0x67);
const ALERT: (u8, u8, u8) = (0xE0, 0x6C, 0x75);

/// How to lay out a text report.
#[derive(Clone, Debug)]
pub struct ReportOptions {
    /// Maximum width of a line in characters. Longer lines are ellipsized, never wrapped: a
    /// report that reflows is unreadable over a serial console and awkward to parse.
    pub width: usize,
    /// Emit ANSI colour. Off by default so that pipes, logs and dumb terminals agree.
    pub colour: bool,
    /// How many process rows to list. `0` means every matching process.
    pub process_rows: usize,
    /// Case-insensitive process filter; empty means all of them.
    pub filter: String,
    /// Process ordering.
    pub sort: ProcessSort,
}

impl ReportOptions {
    /// The default report: 78 columns, no colour, twelve processes.
    pub fn plain() -> Self {
        Self {
            width: 78,
            colour: false,
            process_rows: 12,
            filter: String::new(),
            sort: ProcessSort::Memory,
        }
    }

    /// The same report with colour, for an interactive terminal.
    pub fn coloured() -> Self {
        Self {
            colour: true,
            ..Self::plain()
        }
    }
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self::plain()
    }
}

/// Render a reading as a plain-text report.
pub fn text(snapshot: &Snapshot, options: &ReportOptions) -> String {
    // A width too narrow to hold even one column of numbers gets a floor, rather than
    // producing a report made entirely of ellipses.
    let width = options.width.max(24);
    let mut out = String::new();

    push(
        &mut out,
        &identity_line(snapshot),
        width,
        accent(options, MINT),
    );
    push(&mut out, &hardware_line(snapshot), width, None);
    blank(&mut out);
    cpu_section(&mut out, snapshot, width, options);
    memory_section(&mut out, snapshot, width, options);
    disk_section(&mut out, snapshot, width, options);
    network_section(&mut out, snapshot, width, options);
    thermal_section(&mut out, snapshot, width, options);
    process_section(&mut out, snapshot, width, options);
    out
}

/// Render a reading as JSON, for scripts and for the sample logs.
///
/// The numbers here are raw rather than prettified: bytes stay bytes, durations stay seconds
/// and percentages stay percentages, so a caller never has to undo a "MiB" suffix. `null`
/// means "the platform would not say" — an absent temperature, an unknown user, a process
/// with no parent — and is deliberately distinct from `0`.
///
/// The process list is complete and ordered by memory: a script can filter and truncate far
/// more cheaply than the collector can guess what it wanted.
pub fn json(snapshot: &Snapshot) -> String {
    let fields: Vec<(&str, String)> = vec![
        ("generated_by", string("morrow-watch")),
        ("version", string(env!("CARGO_PKG_VERSION"))),
        ("host", host_json(snapshot)),
        ("cpu", cpu_json(snapshot)),
        ("memory", memory_json(snapshot)),
        ("load", load_json(snapshot)),
        (
            "disks",
            array(snapshot.real_disks().into_iter().map(disk_json)),
        ),
        (
            "networks",
            array(snapshot.active_networks().into_iter().map(network_json)),
        ),
        (
            "thermals",
            array(snapshot.readable_thermals().into_iter().map(thermal_json)),
        ),
        (
            "processes",
            array(
                snapshot
                    .processes_matching("", ProcessSort::Memory)
                    .into_iter()
                    .map(|process| process_json(snapshot, process)),
            ),
        ),
    ];
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("  {}: {}", string(key), value))
        .collect();
    format!("{{\n{}\n}}\n", body.join(",\n"))
}

/// A JSON list. Empty lists are written as `[]`, so a shell never has to special-case them.
fn array(items: impl Iterator<Item = String>) -> String {
    let collected: Vec<String> = items.collect();
    if collected.is_empty() {
        return "[]".to_string();
    }
    format!("[\n    {}\n  ]", collected.join(",\n    "))
}

/// A JSON string literal, escaped as RFC 8259 requires.
///
/// Process names are attacker-adjacent input as far as this is concerned: a binary called
/// `we"ird\n` must not be able to break the document it is reported in.
fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// An optional string, written as `null` when the platform would not say.
fn maybe_string(value: Option<&str>) -> String {
    match value {
        Some(value) => string(value),
        None => "null".to_string(),
    }
}

/// A 32-bit count, written as `null` when there is none.
fn maybe_u32(value: Option<u32>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

/// An optional count, written as `null` when the platform would not say.
fn maybe_usize(value: Option<usize>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

/// A percentage with one round hundredth, written as `null` when it is not finite.
fn number(value: f32) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "null".to_string()
    }
}

/// An optional percentage, written as `null` when there is none.
fn maybe_number(value: Option<f32>) -> String {
    value.map(number).unwrap_or_else(|| "null".to_string())
}

/// A rate or load average, written as `null` when it is not finite.
fn number64(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "null".to_string()
    }
}

/// A list of percentages on one line, so that one reading is one line in a sample log.
fn floats(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| number(*value))
        .collect::<Vec<String>>()
        .join(", ")
}

/// Who this machine is and how long it has been up.
fn identity_line(snapshot: &Snapshot) -> String {
    format!(
        "MORROW / SYSTEM WATCH  ·  {}  ·  {}  ·  up {}",
        snapshot.host.host_name,
        snapshot.host.os,
        util::duration_long(snapshot.host.uptime)
    )
}

/// What it is made of.
fn hardware_line(snapshot: &Snapshot) -> String {
    let host = &snapshot.host;
    let cores = match host.physical_cores {
        // Hyperthreading is worth calling out; a plain "2 cores (2)" is not.
        Some(physical) if physical > 0 && physical != host.logical_cores => {
            format!("{physical} cores ({})", host.logical_cores)
        }
        _ => format!("{} cores", host.logical_cores),
    };
    format!("kernel {}  ·  {cores}", host.kernel)
}

/// Append one line, ellipsized to `width` and optionally tinted as a whole.
///
/// Truncation happens *before* tinting, so an ANSI escape can never be cut in half.
fn push(out: &mut String, line: &str, width: usize, rgb: Option<(u8, u8, u8)>) {
    let clipped = util::ellipsize(line, width);
    match rgb {
        Some(rgb) => out.push_str(&tint(&clipped, rgb)),
        None => out.push_str(&clipped),
    }
    out.push('\n');
}

fn blank(out: &mut String) {
    out.push('\n');
}

/// Wrap text in an ANSI 256-colour escape, for terminals that asked for colour.
fn tint(text: &str, rgb: (u8, u8, u8)) -> String {
    format!("\u{1b}[38;5;{}m{text}\u{1b}[0m", util::ansi_256(rgb))
}

/// Column widths for the disk rows.
const MOUNT_WIDTH: usize = 14;
const DISK_METER_WIDTH: usize = 14;
const FILE_SYSTEM_WIDTH: usize = 8;

/// Interface counters, with idle links drawn quietly so the busy one stands out.
const IDLE: (u8, u8, u8) = (0x8A, 0x91, 0x98);

/// Width of the name column in the network and thermal rows.
const NAMED_WIDTH: usize = 12;

/// Interface counters, with idle links drawn quietly so that the one that is busy stands out.
fn network_section(out: &mut String, snapshot: &Snapshot, width: usize, options: &ReportOptions) {
    push(out, "NETWORK", width, accent(options, MINT));
    let networks = snapshot.active_networks();
    if networks.is_empty() {
        push(out, "  no active interface", width, None);
        return;
    }
    for network in networks {
        let busy = network.bytes_per_second() > 0.0;
        push(
            out,
            &format!(
                "  {}  ↓ {}  ↑ {}  ·  {} in / {} out",
                util::pad_right(&util::ellipsize(&network.name, NAMED_WIDTH), NAMED_WIDTH),
                util::rate(network.received_per_second),
                util::rate(network.transmitted_per_second),
                util::bytes(network.total_received),
                util::bytes(network.total_transmitted),
            ),
            width,
            accent(options, if busy { MINT } else { IDLE }),
        );
    }
}

/// Thermal zones, as a share of each zone's own maximum.
///
/// A 45 °C disk and a 45 °C CPU are not equally worrying, so the raw figure is shown beside a
/// percentage of that sensor's own ceiling rather than against one shared scale.
fn thermal_section(out: &mut String, snapshot: &Snapshot, width: usize, options: &ReportOptions) {
    push(out, "THERMALS", width, accent(options, MINT));
    let thermals = snapshot.readable_thermals();
    if thermals.is_empty() {
        push(out, "  no readable sensor", width, None);
        return;
    }
    for thermal in thermals {
        let current = thermal.celsius.unwrap_or(0.0);
        let percent = util::clamp_percent(thermal.percent_of_max());
        let ceiling = match thermal.max_celsius {
            Some(max) if max.is_finite() && max > 0.0 => format!(" of {max:.0} °C"),
            _ => String::new(),
        };
        push(
            out,
            &format!(
                "  {}  {current:.1} °C{}  {}  {}",
                util::pad_right(&util::ellipsize(&thermal.label, NAMED_WIDTH), NAMED_WIDTH),
                ceiling,
                util::pad_left(&util::percent(percent), 6),
                util::bar(percent, 100.0, 12),
            ),
            width,
            accent(options, load_colour(percent)),
        );
        // The critical trip point explains a machine that has started to throttle, so it is
        // printed as a footnote, and only once the zone is genuinely hot.
        if percent >= 80.0 {
            if let Some(critical) = thermal
                .critical_celsius
                .filter(|value| value.is_finite() && *value > 0.0)
            {
                push(
                    out,
                    &format!("      critical at {critical:.0} °C"),
                    width,
                    accent(options, ALERT),
                );
            }
        }
    }
}

/// Resolve a section accent against the caller's colour preference.
fn accent(options: &ReportOptions, rgb: (u8, u8, u8)) -> Option<(u8, u8, u8)> {
    if options.colour {
        Some(rgb)
    } else {
        None
    }
}

/// The colour a load should be drawn in: calm, warm, or alarming.
fn load_colour(percent: f32) -> (u8, u8, u8) {
    if percent >= 90.0 {
        ALERT
    } else if percent >= 70.0 {
        AMBER
    } else {
        MINT
    }
}

/// The identity of the machine.
fn host_json(snapshot: &Snapshot) -> String {
    let host = &snapshot.host;
    format!(
        "{{\"name\": {}, \"os\": {}, \"kernel\": {}, \"uptime_seconds\": {}, \
         \"physical_cores\": {}, \"logical_cores\": {}}}",
        string(&host.host_name),
        string(&host.os),
        string(&host.kernel),
        host.uptime,
        maybe_usize(host.physical_cores),
        host.logical_cores,
    )
}

/// Processor load and identity, with the per-core split and the recent history.
fn cpu_json(snapshot: &Snapshot) -> String {
    let cpu = &snapshot.cpu;
    format!(
        "{{\"usage_percent\": {}, \"per_core_percent\": [{}], \"history_percent\": [{}], \
         \"brand\": {}, \"frequency_mhz\": {}}}",
        number(cpu.usage),
        floats(&cpu.per_core),
        floats(&snapshot.cpu_history),
        string(&cpu.brand),
        cpu.frequency_mhz,
    )
}

/// RAM and swap, in bytes.
fn memory_json(snapshot: &Snapshot) -> String {
    let memory = &snapshot.memory;
    format!(
        "{{\"total_bytes\": {}, \"used_bytes\": {}, \"available_bytes\": {}, \
         \"used_percent\": {}, \"swap_total_bytes\": {}, \"swap_used_bytes\": {}, \
         \"swap_used_percent\": {}, \"under_pressure\": {}}}",
        memory.total,
        memory.used,
        memory.available,
        number(memory.percent_used()),
        memory.swap_total,
        memory.swap_used,
        number(memory.swap_percent_used()),
        memory.is_under_pressure(),
    )
}

/// The load averages, raw and relative to the core count.
fn load_json(snapshot: &Snapshot) -> String {
    let load = snapshot.load;
    format!(
        "{{\"one\": {}, \"five\": {}, \"fifteen\": {}, \"relative_percent\": {}}}",
        number64(load.one),
        number64(load.five),
        number64(load.fifteen),
        number(load.relative_to(snapshot.host.logical_cores)),
    )
}

/// One mount point.
fn disk_json(disk: &DiskSample) -> String {
    format!(
        "{{\"name\": {}, \"mount_point\": {}, \"file_system\": {}, \"total_bytes\": {}, \
         \"used_bytes\": {}, \"available_bytes\": {}, \"used_percent\": {}, \"removable\": {}}}",
        string(&disk.name),
        string(&disk.mount_point),
        string(&disk.file_system),
        disk.total,
        disk.used(),
        disk.available,
        number(disk.percent_used()),
        disk.removable,
    )
}

/// One interface, with both the current rate and the running totals, since a rate alone
/// cannot be summed over a log.
fn network_json(network: &NetworkSample) -> String {
    format!(
        "{{\"name\": {}, \"received_per_second\": {}, \"transmitted_per_second\": {}, \
         \"total_received_bytes\": {}, \"total_transmitted_bytes\": {}}}",
        string(&network.name),
        number64(network.received_per_second),
        number64(network.transmitted_per_second),
        network.total_received,
        network.total_transmitted,
    )
}

/// One thermal zone, in Celsius and as a share of its own ceiling.
fn thermal_json(thermal: &ThermalSample) -> String {
    format!(
        "{{\"label\": {}, \"celsius\": {}, \"max_celsius\": {}, \"critical_celsius\": {}, \
         \"percent_of_max\": {}}}",
        string(&thermal.label),
        maybe_number(thermal.celsius),
        maybe_number(thermal.max_celsius),
        maybe_number(thermal.critical_celsius),
        number(thermal.percent_of_max()),
    )
}

/// One process, with its user, its runtime and its share of physical memory.
fn process_json(snapshot: &Snapshot, process: &ProcessSample) -> String {
    format!(
        "{{\"pid\": {}, \"parent_pid\": {}, \"name\": {}, \"user\": {}, \"cpu_percent\": {}, \
         \"memory_bytes\": {}, \"memory_percent\": {}, \"run_time_seconds\": {}, \"status\": {}}}",
        process.pid,
        maybe_u32(process.parent),
        string(&process.name),
        maybe_string(process.user.as_deref()),
        number(process.cpu),
        process.memory,
        number(snapshot.process_memory_percent(process)),
        process.run_time,
        string(&process.status),
    )
}

/// Column widths for the process rows.
const PID_WIDTH: usize = 6;
const USER_WIDTH: usize = 8;
const CPU_WIDTH: usize = 6;
const MEMORY_WIDTH: usize = 6;
const TIME_WIDTH: usize = 9;
const STATE_WIDTH: usize = 8;

/// Everything in a process row except the name: two leading spaces, every padded column, and
/// the two-space gaps between them. The name column gets whatever is left.
const PROCESS_PREFIX: usize = 2
    + PID_WIDTH
    + 2
    + USER_WIDTH
    + 2
    + CPU_WIDTH
    + 2
    + MEMORY_WIDTH
    + 2
    + TIME_WIDTH
    + 2
    + STATE_WIDTH
    + 2;

/// The running processes: filtered, ordered, and cut to the row budget.
fn process_section(out: &mut String, snapshot: &Snapshot, width: usize, options: &ReportOptions) {
    let matching = snapshot.processes_matching(&options.filter, options.sort);
    // A budget of zero means "all of them": the flag exists to fit a screen, not to hide work.
    let shown = if options.process_rows == 0 {
        matching.len()
    } else {
        matching.len().min(options.process_rows)
    };
    let needle = options.filter.trim();
    push(
        out,
        &format!(
            "PROCESSES  sorted by {}  ·  filter: {}  ·  showing {} of {}",
            sort_word(options.sort),
            if needle.is_empty() {
                "none".to_string()
            } else {
                util::ellipsize(needle, 24)
            },
            shown,
            matching.len()
        ),
        width,
        accent(options, MINT),
    );
    if matching.is_empty() {
        push(out, "  nothing matched", width, None);
        return;
    }

    // The name column takes the rest of the line, with a floor so that a very narrow terminal
    // still shows something recognisable rather than a column of ellipses.
    let name_width = width.saturating_sub(PROCESS_PREFIX).max(8);
    push(
        out,
        &format!(
            "  {}  {}  {}  {}  {}  {}  {}",
            util::pad_left("PID", PID_WIDTH),
            util::pad_right("USER", USER_WIDTH),
            util::pad_left("CPU%", CPU_WIDTH),
            util::pad_left("MEM%", MEMORY_WIDTH),
            util::pad_right("TIME", TIME_WIDTH),
            util::pad_right("STATE", STATE_WIDTH),
            "NAME",
        ),
        width,
        None,
    );
    for process in matching.iter().take(shown) {
        push(
            out,
            &format!(
                "  {}  {}  {}  {}  {}  {}  {}",
                util::pad_left(&process.pid.to_string(), PID_WIDTH),
                util::pad_right(
                    &util::ellipsize(process.user.as_deref().unwrap_or("?"), USER_WIDTH),
                    USER_WIDTH
                ),
                util::pad_left(&cpu_figure(process.cpu), CPU_WIDTH),
                util::pad_left(
                    &util::percent(snapshot.process_memory_percent(process)),
                    MEMORY_WIDTH
                ),
                util::pad_right(
                    &util::ellipsize(&util::duration(process.run_time), TIME_WIDTH),
                    TIME_WIDTH
                ),
                util::pad_right(&util::ellipsize(&process.status, STATE_WIDTH), STATE_WIDTH),
                util::ellipsize(&process.name, name_width),
            ),
            width,
            if process.cpu >= 50.0 {
                accent(options, AMBER)
            } else {
                None
            },
        );
    }
}

/// The word used for each ordering, so the header says what the table is sorted on.
fn sort_word(sort: ProcessSort) -> &'static str {
    match sort {
        ProcessSort::Memory => "memory",
        ProcessSort::Cpu => "cpu",
        ProcessSort::Name => "name",
    }
}

/// A process's CPU share, deliberately *not* clamped to 100%.
///
/// sysinfo measures against a single core, as `top` does, so a process saturating both cores
/// of a 2-core machine reads as 200%. Clamping that to 100% would describe the machine
/// honestly and the process deceptively.
fn cpu_figure(percent: f32) -> String {
    let value = if percent.is_finite() {
        percent.max(0.0)
    } else {
        0.0
    };
    format!("{value:.1}%")
}

/// Meter width for the CPU, memory and disk rows: a quarter of the line, within reason.
fn meter_width(width: usize) -> usize {
    (width / 4).clamp(8, 24)
}

/// Sparkline width for the CPU row: whatever is left after the label, figure and meter.
fn spark_width(width: usize) -> usize {
    width.saturating_sub(19 + meter_width(width)).min(60)
}

/// CPU load, its recent history, and the load averages.
fn cpu_section(out: &mut String, snapshot: &Snapshot, width: usize, options: &ReportOptions) {
    let cpu = &snapshot.cpu;
    let usage = util::clamp_percent(cpu.usage);
    let line = format!(
        "CPU      {}  {}  {}",
        util::pad_left(&util::percent(usage), 6),
        util::bar(usage, 100.0, meter_width(width)),
        util::sparkline(&snapshot.cpu_history, spark_width(width))
    );
    push(out, &line, width, accent(options, load_colour(usage)));

    let mut facts: Vec<String> = Vec::new();
    if !cpu.brand.is_empty() {
        facts.push(cpu.brand.clone());
    }
    if cpu.frequency_mhz > 0 {
        facts.push(format!("{} MHz", cpu.frequency_mhz));
    }
    let load = snapshot.load;
    if load.one > 0.0 || load.five > 0.0 || load.fifteen > 0.0 {
        // Load only means something in units of cores: 1.0 on a two-core machine is half
        // busy, and on a single-core machine it is completely busy.
        facts.push(format!(
            "load {:.2} {:.2} {:.2} ({:.0}% of {} cores)",
            load.one,
            load.five,
            load.fifteen,
            load.relative_to(snapshot.host.logical_cores),
            snapshot.host.logical_cores
        ));
    }
    // The per-core split, when there is more than one core to split. On the target machine
    // this is the line that answers "is it busy, or is one core pinned?".
    if cpu.per_core.len() > 1 {
        let cores = cpu
            .per_core
            .iter()
            .map(|value| util::percent(*value))
            .collect::<Vec<String>>()
            .join("  ");
        push(out, &format!("         cores {cores}"), width, None);
    }
    if !facts.is_empty() {
        push(
            out,
            &format!("         {}", facts.join("  ·  ")),
            width,
            None,
        );
    }
}

/// RAM and swap, with a warning when the machine is close to thrashing.
fn memory_section(out: &mut String, snapshot: &Snapshot, width: usize, options: &ReportOptions) {
    let memory = &snapshot.memory;
    let meter = meter_width(width);
    let used_percent = util::clamp_percent(memory.percent_used());
    push(
        out,
        &format!(
            "MEMORY   {}  {}  {} of {} used  ·  {} free",
            util::pad_left(&util::percent(used_percent), 6),
            util::bar(used_percent, 100.0, meter),
            util::bytes(memory.used),
            util::bytes(memory.total),
            util::bytes(memory.available),
        ),
        width,
        accent(options, load_colour(used_percent)),
    );

    // A machine with no swap gets no swap row: a permanent "0 B of 0 B" is noise, and noise
    // teaches people to skip the section that matters.
    if memory.swap_total > 0 {
        let swap_percent = util::clamp_percent(memory.swap_percent_used());
        push(
            out,
            &format!(
                "SWAP     {}  {}  {} of {} used",
                util::pad_left(&util::percent(swap_percent), 6),
                util::bar(swap_percent, 100.0, meter),
                util::bytes(memory.swap_used),
                util::bytes(memory.swap_total),
            ),
            width,
            accent(options, load_colour(swap_percent)),
        );
    }

    if memory.is_under_pressure() {
        push(
            out,
            "! under memory pressure: expect things to feel slow rather than frozen",
            width,
            accent(options, ALERT),
        );
    }
}

/// Mount points, real storage only, `/` first because they are listed in mount point order.
fn disk_section(out: &mut String, snapshot: &Snapshot, width: usize, options: &ReportOptions) {
    push(out, "DISKS", width, accent(options, MINT));
    let disks = snapshot.real_disks();
    if disks.is_empty() {
        push(out, "  no storage reported", width, None);
        return;
    }
    for disk in disks {
        let used_percent = util::clamp_percent(disk.percent_used());
        let filesystem = util::ellipsize(&disk.file_system, FILE_SYSTEM_WIDTH);
        let suffix = if filesystem.is_empty() {
            String::new()
        } else {
            format!("  {filesystem}")
        };
        push(
            out,
            &format!(
                "  {}  {}  {}  {} free of {}{}",
                util::pad_right(
                    &util::ellipsize(&disk.mount_point, MOUNT_WIDTH),
                    MOUNT_WIDTH
                ),
                util::bar(used_percent, 100.0, DISK_METER_WIDTH),
                util::pad_left(&util::percent(used_percent), 6),
                util::bytes(disk.available),
                util::bytes(disk.total),
                suffix,
            ),
            width,
            accent(options, load_colour(used_percent)),
        );
        // Only removable media get a second line: on the target machine every disk is an
        // obvious partition of the one device the ISO booted from, and doubling every row
        // would push the process table off the screen.
        if disk.removable {
            push(
                out,
                &format!("      removable  ·  {}", disk.name),
                width,
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{CpuSample, HostSample, LoadSample, MemorySample};

    /// The shape of the target machine: two cores, 500 MiB of RAM, one disk, one interface,
    /// one sensor, and a process list with something to sort, something to hide and
    /// something for a filter to find.
    fn target_machine() -> Snapshot {
        Snapshot {
            host: HostSample {
                host_name: "morrow".to_string(),
                os: "Alpine Linux 3.21.0".to_string(),
                kernel: "Linux 6.12.4-lts".to_string(),
                uptime: 366_100,
                physical_cores: Some(2),
                logical_cores: 2,
            },
            cpu: CpuSample {
                usage: 38.2,
                per_core: vec![44.0, 32.0],
                brand: "Intel(R) Celeron(R) N4020".to_string(),
                frequency_mhz: 1100,
            },
            memory: MemorySample {
                total: 500 * 1024 * 1024,
                used: 205 * 1024 * 1024,
                available: 295 * 1024 * 1024,
                swap_total: 0,
                swap_used: 0,
            },
            load: LoadSample {
                one: 0.42,
                five: 0.31,
                fifteen: 0.24,
            },
            disks: vec![
                DiskSample {
                    name: "/dev/sda1".to_string(),
                    mount_point: "/".to_string(),
                    file_system: "ext4".to_string(),
                    total: 8 * 1024 * 1024 * 1024,
                    available: 4 * 1024 * 1024 * 1024,
                    removable: false,
                },
                // Kernel plumbing: present in the reading, absent from the report.
                DiskSample {
                    name: "none".to_string(),
                    mount_point: "/sys/fs/cgroup".to_string(),
                    file_system: "cgroup2".to_string(),
                    total: 64 * 1024,
                    available: 0,
                    removable: false,
                },
            ],
            networks: vec![
                NetworkSample {
                    name: "eth0".to_string(),
                    received_per_second: 2457.6,
                    transmitted_per_second: 512.0,
                    total_received: 125_829_120,
                    total_transmitted: 18_874_368,
                },
                NetworkSample {
                    name: "lo".to_string(),
                    received_per_second: 1_048_576.0,
                    transmitted_per_second: 0.0,
                    total_received: 1_000_000,
                    total_transmitted: 1_000_000,
                },
            ],
            thermals: vec![ThermalSample {
                label: "x86_pkg_temp".to_string(),
                celsius: Some(45.0),
                max_celsius: Some(90.0),
                critical_celsius: Some(100.0),
            }],
            processes: vec![
                process(1, "init", 0.0, 1024 * 1024),
                process(4242, "morrow-watch", 12.5, 40 * 1024 * 1024),
                process(77, "sshd", 0.4, 3 * 1024 * 1024),
            ],
            cpu_history: vec![10.0, 22.0, 38.2],
            memory_history: vec![40.0, 41.0],
        }
    }

    /// A process row with the same unremarkable settings everywhere, so that each test only
    /// has to state the field it actually cares about.
    fn process(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessSample {
        ProcessSample {
            pid,
            parent: Some(1),
            name: name.to_string(),
            cpu,
            memory,
            run_time: 610,
            status: "sleep".to_string(),
            user: Some("root".to_string()),
        }
    }

    /// Remove ANSI colour escapes, so the width guarantee can be checked on coloured output.
    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut escaping = false;
        for ch in text.chars() {
            if escaping {
                if ch == 'm' {
                    escaping = false;
                }
            } else if ch == '\u{1b}' {
                escaping = true;
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// The character width of the widest line, which is what a terminal wraps on.
    fn widest(text: &str) -> usize {
        text.lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn the_report_names_every_part_of_the_machine() {
        let rendered = text(&target_machine(), &ReportOptions::plain());
        for expected in [
            "MORROW / SYSTEM WATCH",
            "morrow",
            "Alpine Linux 3.21.0",
            "up 4 days 5 hours",
            "kernel Linux 6.12.4-lts  ·  2 cores",
            "38.2%",
            "cores 44.0%  32.0%",
            "Intel(R) Celeron(R) N4020",
            "1100 MHz",
            "load 0.42 0.31 0.24",
            "205 MiB of 500 MiB used",
            "295 MiB free",
            "8.00 GiB",
            "ext4",
            "eth0",
            "2.40 KiB/s",
            "512 B/s",
            "x86_pkg_temp",
            "45.0 °C of 90 °C",
            "PROCESSES",
            "morrow-watch",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in\n{rendered}"
            );
        }

        // A terminal as wide as the roomy default gets the load line whole, including the
        // sentence that turns three numbers into a statement about the cores.
        let roomy = text(
            &target_machine(),
            &ReportOptions {
                width: 200,
                ..ReportOptions::plain()
            },
        );
        assert!(
            roomy.contains("load 0.42 0.31 0.24 (21% of 2 cores)"),
            "missing the load reading in\n{roomy}"
        );
    }

    #[test]
    fn the_report_never_exceeds_the_width_it_is_given() {
        let mut machine = target_machine();
        // Names and figures long enough to force truncation at every width under test.
        machine.host.host_name = "a-very-long-hostname-for-a-very-long-machine".to_string();
        machine.host.os = "Alpine Linux 3.21.0 (edge, musl, hardened, with everything)".to_string();
        machine.cpu.brand = "Intel(R) Celeron(R) N4020 CPU @ 1.10GHz (2 cores)".to_string();
        machine.disks[0].mount_point = "/very/long/mount/point/that/keeps/going".to_string();
        machine.processes.push(process(
            9999,
            "a-process-with-a-remarkably-long-name",
            100.0,
            300 * 1024 * 1024,
        ));

        for width in [24, 40, 60, 78, 120, 200] {
            let options = ReportOptions {
                width,
                ..ReportOptions::plain()
            };
            let rendered = text(&machine, &options);
            let widest = widest(&rendered);
            assert!(
                widest <= width,
                "width {width} exceeded by {widest} in\n{rendered}"
            );

            // Colour changes the bytes, never the geometry: a terminal that asked for colour
            // must not get a report that wraps where the plain one did not.
            let coloured = text(
                &machine,
                &ReportOptions {
                    colour: true,
                    ..options.clone()
                },
            );
            assert_eq!(
                plain(&coloured),
                rendered,
                "colour changed the text at width {width}"
            );
        }
    }

    #[test]
    fn a_machine_that_reports_nothing_still_reads_as_a_report() {
        let rendered = text(&Snapshot::default(), &ReportOptions::plain());
        for expected in [
            "MORROW / SYSTEM WATCH",
            "0 cores",
            "no storage reported",
            "no active interface",
            "no readable sensor",
            "nothing matched",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in\n{rendered}"
            );
        }
        // Nothing is invented for a machine that said nothing: no swap row, no warning, and
        // no arithmetic blowing up in the reader's face.
        assert!(!rendered.contains("SWAP"));
        assert!(!rendered.contains("under memory pressure"));
        assert!(!rendered.contains("NaN"));
        assert!(!rendered.contains("inf"));
        assert!(widest(&rendered) <= ReportOptions::plain().width);
    }

    #[test]
    fn memory_lines_appear_only_when_they_are_true() {
        // Room to spare: no swap row to read, and no warning to ignore.
        let calm = text(&target_machine(), &ReportOptions::plain());
        assert!(!calm.contains("SWAP"));
        assert!(!calm.contains("under memory pressure"));

        // A machine that has started to swap says so twice: once as figures, once in words.
        let mut straining = target_machine();
        straining.memory.total = 1024 * 1024 * 1024;
        straining.memory.used = 480 * 1024 * 1024;
        straining.memory.available = 20 * 1024 * 1024;
        straining.memory.swap_total = 1024 * 1024 * 1024;
        straining.memory.swap_used = 600 * 1024 * 1024;
        let rendered = text(&straining, &ReportOptions::plain());
        assert!(rendered.contains("SWAP"), "{rendered}");
        assert!(
            rendered.contains("600 MiB of 1.00 GiB used"),
            "a swap row must be in bytes a person can judge\n{rendered}"
        );
        assert!(rendered.contains("under memory pressure"), "{rendered}");
    }

    #[test]
    fn the_process_table_respects_the_budget_and_the_filter() {
        let machine = target_machine();
        let limited = ReportOptions {
            process_rows: 2,
            ..ReportOptions::plain()
        };
        // Sorted by memory, the two biggest are morrow-watch (40 MiB) and sshd (3 MiB), so
        // init is the row the budget drops.
        let rendered = text(&machine, &limited);
        assert!(rendered.contains("showing 2 of 3"), "{rendered}");
        assert_eq!(rendered.matches("morrow-watch").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("sshd").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("init").count(), 0, "{rendered}");

        // A filter narrows the list, and the header says which filter was used.
        let filtered = ReportOptions {
            filter: "sshd".to_string(),
            ..ReportOptions::plain()
        };
        let rendered = text(&machine, &filtered);
        assert!(rendered.contains("filter: sshd"), "{rendered}");
        assert!(rendered.contains("showing 1 of 1"), "{rendered}");
        assert!(rendered.contains("sshd"), "{rendered}");
        assert!(!rendered.contains("morrow-watch"), "{rendered}");

        // A filter that matches nothing is reported, not silently ignored.
        let miss = ReportOptions {
            filter: "no-such-process".to_string(),
            ..ReportOptions::plain()
        };
        let rendered = text(&machine, &miss);
        assert!(rendered.contains("showing 0 of 0"), "{rendered}");
        assert!(rendered.contains("nothing matched"), "{rendered}");

        // sysinfo measures CPU against a single core, so a process using both cores of the
        // target reads as 200%, and the table must not quietly say 100%.
        assert_eq!(cpu_figure(200.0), "200.0%");
        assert_eq!(cpu_figure(12.5), "12.5%");
        assert_eq!(cpu_figure(f32::NAN), "0.0%");
        assert_eq!(cpu_figure(-5.0), "0.0%");
        let mut hungry = target_machine();
        hungry
            .processes
            .push(process(9_001, "hungry", 200.0, 10 * 1024 * 1024));
        let rendered = text(&hungry, &ReportOptions::plain());
        assert!(rendered.contains("200.0%"), "{rendered}");
    }

    #[test]
    fn json_is_escaped_raw_and_structurally_sound() {
        assert_eq!(string("plain"), "\"plain\"");
        assert_eq!(string("we\"ird\\name"), "\"we\\\"ird\\\\name\"");
        assert_eq!(
            string("two\nlines\tand a \u{1} control"),
            "\"two\\nlines\\tand a \\u0001 control\""
        );

        let mut machine = target_machine();
        machine
            .processes
            .push(process(4_243, "we\"ird\\name", 1.0, 1024));
        // A process that uses both cores of a two-core machine, so the document carries the
        // same unclamped figure the text report prints.
        machine
            .processes
            .push(process(9_001, "hungry", 200.0, 10 * 1024 * 1024));
        let rendered = json(&machine);
        assert!(
            rendered.contains("\"name\": \"we\\\"ird\\\\name\""),
            "a process name must not be able to break the document\n{rendered}"
        );

        // Raw units, not prettified ones: a script should never have to parse "MiB".
        for expected in [
            "\"usage_percent\": 38.20",
            "\"per_core_percent\": [44.00, 32.00]",
            "\"history_percent\": [10.00, 22.00, 38.20]",
            "\"uptime_seconds\": 366100",
            "\"total_bytes\": 524288000",
            "\"available_bytes\": 309329920",
            "\"received_per_second\": 2457.60",
            "\"mount_point\": \"/\"",
            "\"celsius\": 45.00",
            "\"percent_of_max\": 50.00",
            "\"cpu_percent\": 200.00",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in\n{rendered}"
            );
        }

        // Loopback is filtered exactly as it is for the text report, so a script and a person
        // are never shown two different machines.
        assert!(!rendered.contains("\"lo\""), "{rendered}");
        assert!(!rendered.contains("cgroup2"), "{rendered}");

        // Balanced braces and brackets: the document is a document, not a fragment.
        assert_eq!(rendered.matches('{').count(), rendered.matches('}').count());
        assert_eq!(rendered.matches('[').count(), rendered.matches(']').count());
    }

    #[test]
    fn json_reports_absences_as_empty_lists_and_nulls() {
        let rendered = json(&Snapshot::default());
        for expected in [
            "\"generated_by\": \"morrow-watch\"",
            "\"physical_cores\": null",
            "\"under_pressure\": false",
            "\"disks\": []",
            "\"networks\": []",
            "\"thermals\": []",
            "\"processes\": []",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in\n{rendered}"
            );
        }
        assert!(!rendered.contains("NaN"), "{rendered}");

        // A process with no parent and no known user reports both as null, which is not the
        // same statement as zero and not the same statement as an empty string.
        let mut machine = Snapshot::default();
        let mut orphan = process(7, "orphan", 0.0, 0);
        orphan.parent = None;
        orphan.user = None;
        machine.processes.push(orphan);
        let rendered = json(&machine);
        assert!(rendered.contains("\"parent_pid\": null"), "{rendered}");
        assert!(rendered.contains("\"user\": null"), "{rendered}");
        assert!(rendered.contains("\"name\": \"orphan\""), "{rendered}");
        assert!(rendered.contains("\"memory_bytes\": 0"), "{rendered}");
    }
}
