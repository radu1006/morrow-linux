//! Small, dependency-free formatting helpers.
//!
//! Everything here is pure: no I/O, no locale lookups. `LC_ALL` is ignored on purpose so
//! that console output stays stable enough to be parsed by shell scripts, and so that the
//! UI cannot suddenly start printing commas as decimal separators.

/// Binary (1024-based) byte formatting, e.g. `1.50 GiB`.
///
/// We deliberately print KiB/MiB/GiB rather than kB/MB. Those are the units used by
/// busybox `free`/`df`, `/proc/meminfo` consumers and every other tool on the ISO, and
/// mixing the two conventions is the most common way to misread a memory monitor.
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut size = value as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else if size < 10.0 {
        format!("{size:.2} {}", UNITS[unit])
    } else if size < 100.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

/// Bytes per second, e.g. `2.40 MiB/s`.
pub fn rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "0 B/s".to_string();
    }
    format!("{}/s", bytes(bytes_per_second.round() as u64))
}

/// A short, fixed-shape duration: `4d 06h`, `13h 02m`, `07m 41s`, `41s`.
pub fn duration(seconds: u64) -> String {
    let d = seconds / 86_400;
    let h = (seconds % 86_400) / 3_600;
    let m = (seconds % 3_600) / 60;
    let s = seconds % 60;
    if d > 0 {
        format!("{d}d {h:02}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m:02}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// `1 hour 12 minutes` style, used by the uptime footer where there is room to breathe.
pub fn duration_long(seconds: u64) -> String {
    let d = seconds / 86_400;
    let h = (seconds % 86_400) / 3_600;
    let m = (seconds % 3_600) / 60;
    let mut parts: Vec<String> = Vec::new();
    if d > 0 {
        parts.push(format!("{d} day{}", plural(d)));
    }
    if h > 0 {
        parts.push(format!("{h} hour{}", plural(h)));
    }
    if m > 0 && d == 0 {
        parts.push(format!("{m} minute{}", plural(m)));
    }
    if parts.is_empty() {
        return format!("{seconds} seconds");
    }
    parts.join(" ")
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `42.3%` — one decimal, clamped, so columns stay aligned and never read as nonsense.
pub fn percent(value: f32) -> String {
    format!("{:.1}%", clamp_percent(value))
}

/// Clamp a possibly `NaN`/infinite percentage into `0.0..=100.0`.
pub fn clamp_percent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Truncate to `max` characters, appending `…` when something was dropped.
///
/// Counts `char`s rather than bytes so that process names with non-ASCII characters
/// still line up in the console.
pub fn ellipsize(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Right-pad to `width` characters. Never truncates: run the value through [`ellipsize`]
/// first if it may be longer than the column.
pub fn pad_right(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count >= width {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + (width - count));
    out.push_str(text);
    for _ in count..width {
        out.push(' ');
    }
    out
}

/// Left-pad to `width` characters, for right-aligned numeric columns.
pub fn pad_left(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count >= width {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + (width - count));
    for _ in count..width {
        out.push(' ');
    }
    out.push_str(text);
    out
}

/// A unicode meter such as `████████░░░░░░`.
///
/// Only full and light-shade blocks are used: those glyphs exist in every monospace font
/// the ISO ships (`font-dejavu`, `terminus`), unlike the fractional block glyphs that
/// break in the Linux console font.
pub fn bar(value: f32, max: f32, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let ratio = if max <= 0.0 {
        0.0
    } else {
        (value / max).clamp(0.0, 1.0)
    };
    let filled = ((ratio * width as f32).round() as usize).min(width);
    let mut out = String::with_capacity(width * 3);
    for i in 0..width {
        out.push(if i < filled { '█' } else { '░' });
    }
    out
}

/// Sparkline over the last `width` samples of `data`, scaled to `data`'s own peak.
pub fn sparkline(data: &[f32], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if data.is_empty() || width == 0 {
        return String::new();
    }
    let tail = if data.len() > width {
        &data[data.len() - width..]
    } else {
        data
    };
    let peak = tail
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0f32, f32::max)
        .max(1.0);
    tail.iter()
        .map(|v| {
            let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
            let idx = ((v / peak) * (BARS.len() - 1) as f32).round() as usize;
            BARS[idx.min(BARS.len() - 1)]
        })
        .collect()
}

/// Colours handed out by [`stable_color`]. Public so the console renderer, the GUI legend
/// and the tests all agree on one palette instead of duplicating magic numbers.
pub const STABLE_PALETTE: [(u8, u8, u8); 8] = [
    (0x7D, 0xB3, 0xFF),
    (0x8C, 0xE0, 0x9C),
    (0xFF, 0xD1, 0x7A),
    (0xE8, 0x9B, 0xC8),
    (0x7A, 0xD9, 0xD9),
    (0xC9, 0xA8, 0xFF),
    (0xFF, 0x9E, 0x8A),
    (0xB8, 0xE0, 0x82),
];

/// Pick a stable colour for a name (process, disk, interface) so the UI does not shuffle
/// colours between refreshes. Cheap FNV-1a, mapped into a muted palette that stays
/// readable on the dark theme.
pub fn stable_color(seed: &str) -> (u8, u8, u8) {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in seed.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    STABLE_PALETTE[(hash as usize) % STABLE_PALETTE.len()]
}

/// Map an 8-bit RGB triple onto the nearest slot of the 216-colour ANSI cube so the
/// console renderer can emit `\e[38;5;Nm` sequences.
pub fn ansi_256(rgb: (u8, u8, u8)) -> u8 {
    fn level(v: u8) -> u8 {
        // The cube's steps are 0, 95, 135, 175, 215, 255.
        match v {
            0..=47 => 0,
            48..=114 => 1,
            115..=154 => 2,
            155..=194 => 3,
            195..=234 => 4,
            _ => 5,
        }
    }
    16 + 36 * level(rgb.0) + 6 * level(rgb.1) + level(rgb.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_switches_units_at_1024() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.00 KiB");
        // Exactly 1024 KiB is 1 MiB, not "1024.00 KiB".
        assert_eq!(bytes(1024 * 1024), "1.00 MiB");
        assert_eq!(bytes(1536 * 1024), "1.50 MiB");
        assert_eq!(bytes(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }

    #[test]
    fn bytes_precision_shrinks_as_numbers_grow() {
        // Below 10 -> two decimals, below 100 -> one, above -> none.
        assert_eq!(bytes(9 * 1024 + 512), "9.50 KiB");
        assert_eq!(bytes(10 * 1024), "10.0 KiB");
        assert_eq!(bytes(99 * 1024), "99.0 KiB");
        assert_eq!(bytes(100 * 1024), "100 KiB");
        assert_eq!(bytes(500 * 1024 * 1024), "500 MiB");
    }

    #[test]
    fn bytes_stops_at_the_largest_unit() {
        // 1 PiB is the last unit: 2 PiB must not overflow into a missing unit.
        let two_pib = 2 * 1024u64 * 1024 * 1024 * 1024 * 1024;
        assert_eq!(bytes(two_pib), "2.00 PiB");
        // u64::MAX saturates in the top unit instead of wrapping or panicking.
        assert_eq!(bytes(u64::MAX), "16384 PiB");
    }

    #[test]
    fn rate_handles_zero_and_nonsense() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(-5.0), "0 B/s");
        assert_eq!(rate(f64::NAN), "0 B/s");
        assert_eq!(rate(f64::INFINITY), "0 B/s");
        assert_eq!(rate(1536.0), "1.50 KiB/s");
    }

    #[test]
    fn duration_picks_the_coarsest_useful_shape() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(41), "41s");
        assert_eq!(duration(60), "01m 00s");
        assert_eq!(duration(461), "07m 41s");
        assert_eq!(duration(3600), "1h 00m");
        assert_eq!(duration(3600 * 13 + 120), "13h 02m");
        assert_eq!(duration(86_400 * 4 + 3600 * 6), "4d 06h");
    }

    #[test]
    fn duration_is_fixed_width_within_a_band() {
        // Scripts and columns rely on "07m 41s" / "13h 02m" staying 7 characters wide.
        assert_eq!(duration(41), "41s");
        assert_eq!(duration(60), "01m 00s");
        assert_eq!(duration(3_599), "59m 59s");
        assert_eq!(duration(3_600), "1h 00m");
        assert_eq!(duration(86_399), "23h 59m");
        assert_eq!(duration(86_400), "1d 00h");
        assert_eq!(duration(86_400 * 10), "10d 00h");
    }

    #[test]
    fn duration_long_reads_like_english() {
        assert_eq!(duration_long(0), "0 seconds");
        assert_eq!(duration_long(1), "1 seconds");
        assert_eq!(duration_long(3600), "1 hour");
        assert_eq!(duration_long(3600 * 2 + 60), "2 hours 1 minute");
        assert_eq!(duration_long(3600 * 25 + 120), "1 day 1 hour");
        // Minutes are dropped once days are present, to keep the footer short.
        assert_eq!(duration_long(86_400), "1 day");
    }

    #[test]
    fn percent_is_one_decimal_and_clamped() {
        assert_eq!(percent(0.0), "0.0%");
        assert_eq!(percent(42.34), "42.3%");
        assert_eq!(percent(42.36), "42.4%");
        assert_eq!(percent(-1.0), "0.0%");
        assert_eq!(percent(101.0), "100.0%");
        assert_eq!(percent(f32::NAN), "0.0%");
        assert_eq!(percent(f32::INFINITY), "0.0%");
    }

    #[test]
    fn clamp_percent_normalises_only_finite_values() {
        assert_eq!(clamp_percent(50.0), 50.0);
        assert_eq!(clamp_percent(-0.5), 0.0);
        assert_eq!(clamp_percent(150.0), 100.0);
        assert_eq!(clamp_percent(f32::NAN), 0.0);
        assert_eq!(clamp_percent(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn ellipsize_counts_characters_not_bytes() {
        assert_eq!(ellipsize("hello", 10), "hello");
        assert_eq!(ellipsize("hello", 5), "hello");
        assert_eq!(ellipsize("hello world", 5), "hell…");
        assert_eq!(ellipsize("abc", 0), "");
        assert_eq!(ellipsize("", 4), "");
        // "élan vital" is 10 chars but 11 bytes: the cut has to land on char 4, not byte 4.
        assert_eq!(ellipsize("élan vital", 5), "élan…");
        // A multi-byte char still counts as one column, so 1 wide leaves only the ellipsis.
        assert_eq!(ellipsize("héllo", 1), "…");
    }

    #[test]
    fn padding_never_truncates() {
        assert_eq!(pad_right("ab", 5), "ab   ");
        assert_eq!(pad_right("abcde", 5), "abcde");
        assert_eq!(pad_right("abcdef", 3), "abcdef");
        assert_eq!(pad_left("42", 5), "   42");
        assert_eq!(pad_left("abcdef", 2), "abcdef");
    }

    #[test]
    fn padding_uses_character_width() {
        // "é" is two bytes but one column: padding must not come up short by a space.
        assert_eq!(pad_right("é", 3), "é  ");
        assert_eq!(pad_right("é", 3).chars().count(), 3);
        assert_eq!(pad_left("é", 3), "  é");
        assert_eq!(pad_left("é", 3).chars().count(), 3);
    }

    #[test]
    fn bar_fills_proportionally() {
        assert_eq!(bar(50.0, 100.0, 10), "█████░░░░░");
        assert_eq!(bar(0.0, 100.0, 10), "░░░░░░░░░░");
        assert_eq!(bar(100.0, 100.0, 4), "████");
        assert_eq!(bar(75.0, 100.0, 4), "███░");
        assert_eq!(bar(0.0, 0.0, 3), "░░░");
        assert_eq!(bar(1.0, 0.0, 3), "░░░");
        assert_eq!(bar(0.0, 100.0, 0), "");
        assert_eq!(bar(50.0, 100.0, 6).chars().count(), 6);
    }

    #[test]
    fn bar_clamps_out_of_range_input() {
        assert_eq!(bar(200.0, 100.0, 4), "████");
        assert_eq!(bar(-50.0, 100.0, 4), "░░░░");
        // A NaN ratio must degrade to an empty bar rather than panicking on the cast.
        assert_eq!(bar(f32::NAN, 100.0, 4), "░░░░");
    }

    #[test]
    fn sparkline_uses_only_the_tail_it_can_draw() {
        assert_eq!(sparkline(&[], 4), "");
        assert_eq!(sparkline(&[1.0], 0), "");
        assert_eq!(sparkline(&[0.0, 0.0, 0.0, 0.0], 4), "▁▁▁▁");
        // Peak-relative scaling: the largest sample is always a full block.
        assert_eq!(sparkline(&[1.0, 2.0, 4.0], 3), "▃▅█");
        // With a peak of 4, a sample of 1 sits a quarter of the way up the ramp.
        assert_eq!(sparkline(&[4.0, 1.0], 2), "█▃");
        // Older samples are dropped when more data arrives than the width can hold.
        assert_eq!(sparkline(&[0.0, 0.0, 8.0], 1), "█");
    }

    #[test]
    fn sparkline_survives_nonsense_samples() {
        // NaN and negative samples must not corrupt the ramp or panic.
        assert_eq!(sparkline(&[f32::NAN, -1.0, 3.0], 3), "▁▁█");
        // An infinite sample is ignored when the peak is picked, then drawn as empty.
        assert_eq!(sparkline(&[f32::INFINITY, 2.0], 2), "▁█");
    }

    #[test]
    fn stable_colors_are_deterministic_and_from_the_palette() {
        for seed in ["cpu", "memory", "sda", "eth0", "init", ""] {
            let color = stable_color(seed);
            assert!(STABLE_PALETTE.contains(&color), "{seed} produced {color:?}");
            assert_eq!(color, stable_color(seed));
        }
    }

    #[test]
    fn every_palette_entry_is_reachable() {
        // Guards against a hash that only ever lands on one or two colours.
        let mut seen = vec![false; STABLE_PALETTE.len()];
        for index in 0..2048 {
            let seed = format!("iface{index}");
            let slot = STABLE_PALETTE
                .iter()
                .position(|entry| *entry == stable_color(&seed))
                .expect("stable_color only returns palette entries");
            seen[slot] = true;
        }
        assert!(
            seen.iter().all(|hit| *hit),
            "unreached palette slots: {seen:?}"
        );
    }

    #[test]
    fn ansi_256_maps_corners_of_the_cube() {
        assert_eq!(ansi_256((0, 0, 0)), 16);
        assert_eq!(ansi_256((255, 0, 0)), 196);
        assert_eq!(ansi_256((0, 255, 0)), 46);
        assert_eq!(ansi_256((0, 0, 255)), 21);
        assert_eq!(ansi_256((255, 255, 255)), 231);
    }

    #[test]
    fn ansi_256_bands_are_inclusive_at_the_edges() {
        // Red-only triples isolate the red band: 16 + 36 * level.
        assert_eq!(ansi_256((47, 0, 0)), 16);
        assert_eq!(ansi_256((48, 0, 0)), 52);
        assert_eq!(ansi_256((114, 0, 0)), 52);
        assert_eq!(ansi_256((115, 0, 0)), 88);
        assert_eq!(ansi_256((194, 0, 0)), 124);
        assert_eq!(ansi_256((195, 0, 0)), 160);
        assert_eq!(ansi_256((234, 0, 0)), 160);
        assert_eq!(ansi_256((235, 0, 0)), 196);
    }
}
