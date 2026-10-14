//! The `morrow-watch` binary: argument handling, and the choice of front end.
//!
//! Everything that decides *what* is shown lives in the library, where it is tested without a
//! display, a terminal or a real 500 MB machine. This file only decides which front end runs,
//! and turns the reading into bytes on stdout or into a window.

#[cfg(feature = "gui")]
mod gui;

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::thread;

use morrow_watch::cli::{self, Action, Mode, Options};
use morrow_watch::collector::Collector;
use morrow_watch::report;

fn main() -> ExitCode {
    match cli::parse(std::env::args().skip(1)) {
        Ok(Action::Run(options)) => run(options),
        Ok(Action::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Action::Version) => {
            println!("morrow-watch {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Err(message) => {
            // A mistake goes to stderr, so that anything on stdout stays a clean report even
            // when part of the command line was wrong.
            eprintln!("morrow-watch: {message}");
            eprintln!("run `morrow-watch --help` for the options");
            ExitCode::from(2)
        }
    }
}

/// Run the front end the arguments asked for.
fn run(options: Options) -> ExitCode {
    match options.mode {
        Mode::Text => print_reports(&options),
        Mode::Json => print_json(&options),
        Mode::Gui => open_window(options),
    }
}

/// Write one report, or keep writing them until the count runs out.
///
/// A count of zero means "until interrupted", which is what `--interval` on its own asks for.
fn print_reports(options: &Options) -> ExitCode {
    // One collector for the whole run: it holds the machine's previous CPU and network
    // counters, so a collector rebuilt for every report would have nothing to subtract from and
    // would read 0% busy forever.
    let mut collector = Collector::new();
    // Only this file can see what stdout is, so `Auto` is settled here and the renderer is
    // simply told the answer.
    let colour = Options::resolve_colour(options.colour, io::stdout().is_terminal());
    let report_options = options.report_options(colour);
    let tick = options.tick_options();
    let interval = options.interval.unwrap_or_else(Collector::sample_interval);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut written = 0usize;
    loop {
        let snapshot = collector.tick(tick);
        if let Err(error) = out.write_all(report::text(&snapshot, &report_options).as_bytes()) {
            return write_failed(error);
        }
        // Flushed per report: watching through `less`, or appending to a log, should show each
        // reading as it happens rather than in four-kilobyte blocks.
        if let Err(error) = out.flush() {
            return write_failed(error);
        }
        written += 1;
        if options.count != 0 && written >= options.count {
            return ExitCode::SUCCESS;
        }
        thread::sleep(interval);
    }
}

/// Write one JSON document and stop.
///
/// The command line refuses `--interval` and `--count` for JSON, so there is no loop here to
/// get wrong: one run, one document. A script that wants a series calls the program in a loop.
fn print_json(options: &Options) -> ExitCode {
    let mut collector = Collector::new();
    let snapshot = collector.tick(options.tick_options());
    let document = report::json(&snapshot);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Err(error) = out.write_all(document.as_bytes()) {
        return write_failed(error);
    }
    if let Err(error) = out.flush() {
        return write_failed(error);
    }
    ExitCode::SUCCESS
}

/// A write that failed.
///
/// A closed pipe is how `| head -20` ends a live view, so it exits quietly and successfully
/// rather than pretending the machine broke.
fn write_failed(error: io::Error) -> ExitCode {
    if error.kind() == io::ErrorKind::BrokenPipe {
        return ExitCode::SUCCESS;
    }
    eprintln!("morrow-watch: cannot write the report: {error}");
    ExitCode::FAILURE
}

/// Open the desktop window.
#[cfg(feature = "gui")]
fn open_window(options: Options) -> ExitCode {
    gui::run(options)
}

/// A build without the window feature was asked for one.
///
/// It says so and exits rather than silently printing a report, because a desktop entry that
/// wanted a window and got a wall of text in no terminal looks like a crash.
#[cfg(not(feature = "gui"))]
fn open_window(_options: Options) -> ExitCode {
    eprintln!(
        "morrow-watch: this build has no window; rebuild with the `gui` feature, or use --text"
    );
    ExitCode::FAILURE
}
