//! Morrow Watch: the monitoring core shared by Morrow's two front ends.
//!
//! Morrow ships one binary that renders either an egui window (the Openbox desktop) or a
//! plain text/JSON report (servers, rescue shells, scripts). Both front ends need exactly
//! the same numbers, so the logic lives here in the library and the binary only decides how
//! to draw it.
//!
//! The split is deliberate: it keeps every piece of formatting and sampling logic unit
//! testable without an X11 server, an OpenGL context or a real 500 MB machine.

pub mod cli;
pub mod collector;
pub mod report;
pub mod util;
