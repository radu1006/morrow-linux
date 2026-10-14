//! The command line Morrow Watch is driven by.
//!
//! This lives in the library rather than in `main.rs` for one reason: it decides what every
//! front end shows, and rules that are tested are rules that keep working. Nothing here reads
//! a terminal, a file or the machine itself, so the whole surface is covered by unit tests
//! that run without X11, a window or a real 500 MB box.
//!
//! The parser takes an iterator of arguments and returns either a request to run, or an
//! explanation of what was wrong with it. `main.rs` is then only wiring.

use std::time::Duration;

use crate::collector::{ProcessSort, TickOptions};
use crate::report::ReportOptions;

/// The narrowest report worth printing. Below this there is no room for a section header and
/// one figure on the same line, so the report would be nothing but ellipses.
pub const MIN_WIDTH: usize = 24;

/// As wide as any terminal gets. Beyond this the report is mostly padding, and a line that
/// long is a paste into a chat window nobody asked for.
pub const MAX_WIDTH: usize = 512;

/// The most process rows anyone gets to ask for in one report.
pub const MAX_ROWS: usize = 4096;

/// The slowest refresh worth offering a live view.
pub const MAX_INTERVAL_SECS: u64 = 3600;

/// How many reports a single run may be asked for. Zero means "until interrupted".
pub const MAX_COUNT: usize = 100_000;

pub const USAGE: &str = "\
morrow-watch - a small system monitor for Morrow Linux

USAGE
  morrow-watch [OPTIONS]

With no options, morrow-watch opens the desktop window when this build has one and otherwise
prints a single text report.

OUTPUT
  --text                 Print one text report (the default without a window)
  -j, --json             Print one JSON document
  --gui                  Open the desktop window
  -w, --width <N>        Report width in columns, 24..=512 (default 78)
      --colour, --color  Always emit ANSI colour
      --no-colour        Never emit ANSI colour
  -n, --rows <N>         Process rows to list, 0 for every match (default 12, max 4096)
      --filter <TEXT>    Only processes whose name, user or pid contains TEXT
  -s, --sort <KEY>       Order processes by memory (default), cpu or name
      --interval <SECS>  Print a report every SECS seconds, 1..=3600
      --count <N>        Stop after N reports, 0 for until interrupted (default 1)
  -h, --help             Show this text and exit
  -V, --version          Show the version and exit

Colour is chosen automatically: on a terminal, off through a pipe or a log. --json never
carries colour, and one JSON document is always written per run, so --interval is refused
there: run the program again, or loop in the calling script.
";

/// How much colour to put in a report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColourChoice {
    /// Colour when writing to a terminal, never when writing to a pipe.
    Auto,
    /// Colour even through a pipe, for `less -R` and friends.
    Always,
    /// No colour at all.
    Never,
}

/// Which front end to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Text,
    Json,
    Gui,
}

/// A complete request, once every option has been read and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub mode: Mode,
    pub width: usize,
    pub colour: ColourChoice,
    pub rows: usize,
    pub filter: String,
    pub sort: ProcessSort,
    /// `None` means a single report; `Some` means keep going.
    pub interval: Option<Duration>,
    /// How many reports to write. Zero means until interrupted.
    pub count: usize,
}

impl Default for Options {
    fn default() -> Self {
        let report = ReportOptions::plain();
        Self {
            // A build without the window feature cannot open one, so it does the useful thing
            // instead of explaining that it cannot.
            mode: if cfg!(feature = "gui") {
                Mode::Gui
            } else {
                Mode::Text
            },
            width: report.width,
            colour: ColourChoice::Auto,
            rows: report.process_rows,
            filter: report.filter,
            sort: report.sort,
            interval: None,
            count: 1,
        }
    }
}

impl Options {
    /// Whether this request writes more than one report.
    pub fn is_live(&self) -> bool {
        self.interval.is_some() || self.count == 0 || self.count > 1
    }

    /// What to sample: the text and JSON reports print every section, so they ask for
    /// everything. The window asks for less on the page that does not draw processes.
    pub fn tick_options(&self) -> TickOptions {
        TickOptions::everything()
    }

    /// This request as the renderer wants it.
    ///
    /// `colour` is passed in rather than read off `self` because only `main.rs` can see
    /// whether stdout is a terminal, and a library that peeks at stdio cannot be tested.
    pub fn report_options(&self, colour: bool) -> ReportOptions {
        ReportOptions {
            width: self.width,
            colour,
            process_rows: self.rows,
            filter: self.filter.clone(),
            sort: self.sort,
        }
    }

    /// Settle [`ColourChoice::Auto`] against what is known about the output.
    pub fn resolve_colour(choice: ColourChoice, stdout_is_terminal: bool) -> bool {
        match choice {
            ColourChoice::Auto => stdout_is_terminal,
            ColourChoice::Always => true,
            ColourChoice::Never => false,
        }
    }
}

/// What the arguments asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Run the monitor.
    Run(Options),
    /// Print the usage text and exit successfully.
    Help,
    /// Print the version and exit successfully.
    Version,
}

/// Read a command line.
///
/// `--help` and `--version` are answered before anything else is checked: someone asking how
/// the program works should not first have to guess it right, and `morrow-watch --width --help`
/// should explain itself rather than complain about a missing number.
pub fn parse<I>(args: I) -> Result<Action, String>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| ["--help", "-h"].contains(&arg.as_str()))
    {
        return Ok(Action::Help);
    }
    if args
        .iter()
        .any(|arg| ["--version", "-V"].contains(&arg.as_str()))
    {
        return Ok(Action::Version);
    }

    let mut options = Options::default();
    // Kept separate until the end: whether `--count` was asked for at all changes what it
    // defaults to, and `0` is a meaningful value that must not be confused with "unset".
    let mut interval_secs: Option<u64> = None;
    let mut count: Option<usize> = None;

    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].clone();
        let (name, inline) = split_inline(&arg);
        match name {
            "--text" => {
                no_value(inline, name)?;
                options.mode = Mode::Text;
            }
            "-j" | "--json" => {
                no_value(inline, name)?;
                options.mode = Mode::Json;
            }
            "--gui" => {
                no_value(inline, name)?;
                options.mode = Mode::Gui;
            }
            "--colour" | "--color" => {
                no_value(inline, name)?;
                options.colour = ColourChoice::Always;
            }
            "--no-colour" | "--no-color" => {
                no_value(inline, name)?;
                options.colour = ColourChoice::Never;
            }
            "-w" | "--width" => {
                options.width = number(&args, &mut index, inline, name, MIN_WIDTH, MAX_WIDTH)?;
            }
            "-n" | "--rows" => {
                options.rows = number(&args, &mut index, inline, name, 0, MAX_ROWS)?;
            }
            "-s" | "--sort" => {
                options.sort = sort_from(&value(&args, &mut index, inline, name)?)?;
            }
            "--filter" => {
                options.filter = value(&args, &mut index, inline, name)?;
            }
            "--interval" => {
                let secs = number(
                    &args,
                    &mut index,
                    inline,
                    name,
                    1,
                    MAX_INTERVAL_SECS as usize,
                )?;
                interval_secs = Some(secs as u64);
            }
            "--count" => {
                count = Some(number(&args, &mut index, inline, name, 0, MAX_COUNT)?);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}"));
            }
            other => {
                return Err(format!(
                    "unexpected argument {other:?}: morrow-watch takes options, not words"
                ));
            }
        }
        index += 1;
    }

    let interval = interval_secs.map(Duration::from_secs);
    // Asking for a refresh rate means wanting to watch, so a live view keeps going until it is
    // interrupted unless a count says otherwise.
    let count = count.unwrap_or(if interval.is_some() { 0 } else { 1 });

    match options.mode {
        Mode::Json => {
            if interval.is_some() {
                return Err(
                    "--json writes one document per run: run it again for another reading, or \
                     loop in the calling script"
                        .to_string(),
                );
            }
            if count != 1 {
                return Err(
                    "--json writes one document per run: --count does not apply".to_string()
                );
            }
        }
        Mode::Gui => {
            if interval.is_some() {
                return Err("--gui refreshes itself: --interval is for --text".to_string());
            }
            if count != 1 {
                return Err("--gui is a live window: --count is for --text".to_string());
            }
        }
        Mode::Text => {}
    }

    Ok(Action::Run(Options {
        interval,
        count,
        ..options
    }))
}

/// Split `--name=value` into its two halves.
///
/// Only long options are split: `-w=80` is not a form anyone types, and accepting it would
/// make a short option's value ambiguous with anything starting with `=`.
fn split_inline(arg: &str) -> (&str, Option<String>) {
    match arg.split_once('=') {
        Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
        _ => (arg, None),
    }
}

/// The value of an option, written either `--name value` or `--name=value`.
///
/// An empty value is still a value: `--filter=""` means "no filter" and is allowed, so only a
/// genuinely absent argument is an error.
fn value(
    args: &[String],
    index: &mut usize,
    inline: Option<String>,
    name: &str,
) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value);
    }
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} needs a value"))
}

/// Reject a value given to an option that has none, such as `--json=1`.
fn no_value(inline: Option<String>, name: &str) -> Result<(), String> {
    match inline {
        Some(value) => Err(format!("{name} takes no value, but was given {value:?}")),
        None => Ok(()),
    }
}

/// A whole number inside an inclusive range, with an error that says what was wrong and what
/// would have been right.
///
/// A negative or fractional value fails the parse, which is the point: `--width -1` must not
/// be read as four billion columns.
fn number(
    args: &[String],
    index: &mut usize,
    inline: Option<String>,
    name: &str,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let text = value(args, index, inline, name)?;
    let parsed: usize = text
        .trim()
        .parse()
        .map_err(|_| format!("{name} needs a whole number, got {text:?}"))?;
    if parsed < min || parsed > max {
        return Err(format!(
            "{name} must be between {min} and {max}, got {parsed}"
        ));
    }
    Ok(parsed)
}

/// Translate a `--sort` word.
///
/// The short forms are there because `mem` and `rss` are what people type when they mean "the
/// biggest thing in RAM".
fn sort_from(word: &str) -> Result<ProcessSort, String> {
    match word.trim().to_ascii_lowercase().as_str() {
        "memory" | "mem" | "rss" => Ok(ProcessSort::Memory),
        "cpu" => Ok(ProcessSort::Cpu),
        "name" => Ok(ProcessSort::Name),
        other => Err(format!(
            "--sort does not know {other:?}: use memory, cpu or name"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a line the way a shell would have split it.
    fn parse_args(args: &[&str]) -> Result<Action, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    /// The options of a command line that is expected to succeed.
    fn options(args: &[&str]) -> Options {
        match parse_args(args) {
            Ok(Action::Run(options)) => options,
            other => panic!("expected {args:?} to run, got {other:?}"),
        }
    }

    /// The message of a command line that is expected to be refused.
    fn refusal(args: &[&str]) -> String {
        match parse_args(args) {
            Err(message) => message,
            other => panic!("expected {args:?} to be refused, got {other:?}"),
        }
    }

    #[test]
    fn the_default_request_is_one_report_at_the_standard_width() {
        let default = Options::default();
        assert_eq!(default.width, 78);
        assert_eq!(default.rows, 12);
        assert_eq!(default.filter, "");
        assert_eq!(default.sort, ProcessSort::Memory);
        assert_eq!(default.colour, ColourChoice::Auto);
        assert_eq!(default.interval, None);
        assert_eq!(default.count, 1);
        assert!(!default.is_live());
        // A build with a window opens it; a build without one prints rather than complaining.
        let expected = if cfg!(feature = "gui") {
            Mode::Gui
        } else {
            Mode::Text
        };
        assert_eq!(default.mode, expected);
        assert_eq!(parse_args(&[]).unwrap(), Action::Run(default));
    }

    #[test]
    fn every_way_of_reading_the_machine_can_be_asked_for() {
        assert_eq!(options(&["--text"]).mode, Mode::Text);
        assert_eq!(options(&["--json"]).mode, Mode::Json);
        assert_eq!(options(&["-j"]).mode, Mode::Json);
        assert_eq!(options(&["--gui"]).mode, Mode::Gui);
        // The last word wins, so an alias with `--text` in it can be overridden at the prompt.
        assert_eq!(options(&["--json", "--text"]).mode, Mode::Text);
        assert_eq!(options(&["--gui", "--json"]).mode, Mode::Json);
    }

    #[test]
    fn the_width_is_checked_against_what_can_be_printed() {
        assert_eq!(options(&["--width", "40"]).width, 40);
        assert_eq!(options(&["-w", "40"]).width, 40);
        assert_eq!(options(&["--width=40"]).width, 40);
        assert_eq!(options(&["--width", "24"]).width, MIN_WIDTH);
        assert_eq!(options(&["--width", "512"]).width, MAX_WIDTH);

        assert!(refusal(&["--width", "23"]).contains("between 24 and 512"));
        assert!(refusal(&["--width", "513"]).contains("between 24 and 512"));
        // A negative width is not four billion columns, and a word is not a width at all.
        assert!(refusal(&["--width", "-1"]).contains("whole number"));
        assert!(refusal(&["--width", "wide"]).contains("whole number"));
        assert!(refusal(&["--width"]).contains("needs a value"));
    }

    #[test]
    fn colour_is_a_three_way_choice_settled_by_the_output() {
        assert_eq!(options(&[]).colour, ColourChoice::Auto);
        assert_eq!(options(&["--colour"]).colour, ColourChoice::Always);
        assert_eq!(options(&["--color"]).colour, ColourChoice::Always);
        assert_eq!(options(&["--no-colour"]).colour, ColourChoice::Never);
        assert_eq!(options(&["--no-color"]).colour, ColourChoice::Never);
        assert_eq!(
            options(&["--colour", "--no-colour"]).colour,
            ColourChoice::Never
        );
        assert_eq!(
            options(&["--no-colour", "--colour"]).colour,
            ColourChoice::Always
        );

        // Auto means "whatever is on the other end of stdout": a pipe is not a terminal, and
        // an escape sequence in a log file is a bug report.
        assert!(Options::resolve_colour(ColourChoice::Auto, true));
        assert!(!Options::resolve_colour(ColourChoice::Auto, false));
        assert!(Options::resolve_colour(ColourChoice::Always, false));
        assert!(!Options::resolve_colour(ColourChoice::Never, true));
    }

    #[test]
    fn the_process_list_can_be_filtered_and_ordered() {
        assert_eq!(options(&["--filter", "sshd"]).filter, "sshd");
        assert_eq!(options(&["--filter=sshd"]).filter, "sshd");
        // An empty filter means "everything", and is not a missing value.
        assert_eq!(options(&["--filter="]).filter, "");
        assert_eq!(options(&["-s", "cpu"]).sort, ProcessSort::Cpu);
        assert_eq!(options(&["--sort", "name"]).sort, ProcessSort::Name);
        assert_eq!(options(&["--sort", "mem"]).sort, ProcessSort::Memory);
        assert_eq!(options(&["--sort", "RSS"]).sort, ProcessSort::Memory);
        assert!(refusal(&["--sort", "vibes"]).contains("memory, cpu or name"));
        assert!(refusal(&["--sort"]).contains("needs a value"));
    }

    #[test]
    fn the_row_budget_can_be_everything_or_a_window() {
        assert_eq!(options(&["--rows", "0"]).rows, 0);
        assert_eq!(options(&["-n", "3"]).rows, 3);
        assert_eq!(options(&["--rows", "4096"]).rows, MAX_ROWS);
        assert!(refusal(&["--rows", "4097"]).contains("between 0 and 4096"));
        assert!(refusal(&["--rows", "-3"]).contains("whole number"));
    }

    #[test]
    fn a_refresh_rate_means_watching_until_interrupted() {
        // --text throughout: a build with the window feature opens the window by default, and the
        // window refreshes itself, so a rate and a count are what the printed front end offers.
        // What this test is about is the sampling, not which front end was chosen.
        let live = options(&["--text", "--interval", "2"]);
        assert_eq!(live.interval, Some(Duration::from_secs(2)));
        assert_eq!(
            live.count, 0,
            "a live view keeps going until Ctrl-C unless a count says otherwise"
        );
        assert!(live.is_live());

        // A rate with an explicit count of one is a single reading, spaced out.
        let once = options(&["--text", "--interval", "2", "--count", "1"]);
        assert_eq!(once.count, 1);
        assert!(once.is_live());

        // A count without a rate is a burst, paced by the sampler's own second.
        let burst = options(&["--text", "--count", "5"]);
        assert_eq!(burst.interval, None);
        assert_eq!(burst.count, 5);
        assert!(burst.is_live());

        assert!(refusal(&["--text", "--interval", "0"]).contains("between 1 and 3600"));
        assert!(refusal(&["--text", "--interval", "3601"]).contains("between 1 and 3600"));
        assert!(refusal(&["--text", "--interval", "soon"]).contains("whole number"));
    }

    #[test]
    fn one_json_document_per_run() {
        assert_eq!(options(&["--json", "--count", "1"]).count, 1);
        // Colour has no meaning in JSON, so it is accepted and ignored rather than refused.
        assert_eq!(
            options(&["--json", "--colour"]).colour,
            ColourChoice::Always
        );
        assert!(refusal(&["--json", "--interval", "5"]).contains("one document"));
        assert!(refusal(&["--json", "--count", "2"]).contains("one document"));
        assert!(refusal(&["--json", "--count", "0"]).contains("one document"));
    }

    #[test]
    fn the_window_refreshes_itself() {
        assert!(refusal(&["--gui", "--interval", "3"]).contains("refreshes itself"));
        assert!(refusal(&["--gui", "--count", "2"]).contains("live window"));
        // The window does honour the filter and the ordering it was started with.
        assert_eq!(options(&["--gui", "--filter", "sshd"]).filter, "sshd");
        assert_eq!(options(&["--gui", "-s", "cpu"]).sort, ProcessSort::Cpu);
    }

    #[test]
    fn help_and_version_answer_even_after_a_mistake() {
        assert_eq!(parse_args(&["--help"]).unwrap(), Action::Help);
        assert_eq!(parse_args(&["-h"]).unwrap(), Action::Help);
        assert_eq!(parse_args(&["--version"]).unwrap(), Action::Version);
        assert_eq!(parse_args(&["-V"]).unwrap(), Action::Version);
        // Someone asking how the program works should not first have to get it right.
        assert_eq!(
            parse_args(&["--width", "x", "--help"]).unwrap(),
            Action::Help
        );
        assert_eq!(parse_args(&["--nope", "-V"]).unwrap(), Action::Version);
    }

    #[test]
    fn unknown_options_and_stray_words_are_refused() {
        assert!(refusal(&["--nope"]).contains("unknown option \"--nope\""));
        assert!(refusal(&["-x"]).contains("unknown option \"-x\""));
        assert!(refusal(&["report"]).contains("unexpected argument \"report\""));
        // A value given to a flag is worth naming: `--json=1` is a typo for `--count 1`, and
        // ignoring it silently would hide the mistake.
        assert!(refusal(&["--json=1"]).contains("takes no value"));
        assert!(refusal(&["--colour=always"]).contains("takes no value"));
    }

    #[test]
    fn options_become_report_and_sampling_settings() {
        let chosen = options(&[
            "--width", "40", "--rows", "3", "--filter", "sshd", "--sort", "cpu",
        ]);
        let report = chosen.report_options(true);
        assert_eq!(report.width, 40);
        assert_eq!(report.process_rows, 3);
        assert_eq!(report.filter, "sshd");
        assert_eq!(report.sort, ProcessSort::Cpu);
        assert!(report.colour);
        assert!(!chosen.report_options(false).colour);

        // Both printed reports show every section, so the cheapest complete reading is the
        // full one.
        let tick = chosen.tick_options();
        assert!(tick.processes && tick.disks && tick.sensors && tick.network);
    }

    #[test]
    fn the_usage_text_names_every_option_the_parser_accepts() {
        for flag in [
            "--text",
            "--json",
            "-j",
            "--gui",
            "--width",
            "-w",
            "--colour",
            "--color",
            "--no-colour",
            "--rows",
            "-n",
            "--filter",
            "--sort",
            "-s",
            "--interval",
            "--count",
            "--help",
            "-h",
            "--version",
            "-V",
        ] {
            assert!(USAGE.contains(flag), "{flag} is undocumented");
        }
    }
}
