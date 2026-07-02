//! Which "mode" the CLI should enter (spec §6.2, §10.1's first-run
//! branch). Kept as a pure function, separate from `clap` parsing and
//! any actual rendering/fetching, so the branch logic itself can be
//! unit-tested without a terminal or the snapshot engine — "command
//! dispatch" (main.rs wiring this to real subcommands) is verified
//! manually instead, per this task's own test-tier split.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedCommand {
    /// No subcommand given — the default, interactive TUI.
    Default,
    Snapshot,
    Sources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// §10.1's first-run behavior: `sources.yaml` is empty, so open
    /// straight to the Sources screen with a prompt to add one, rather
    /// than fetching against an empty list and rendering a blank
    /// dashboard.
    FirstRunSources,
    Dashboard,
    Sources,
    SnapshotHeadless,
    /// A headless snapshot was requested but nothing is configured to
    /// fetch. Not an error — §10.1's interactive first-run prompt has
    /// no meaning in a non-interactive `launchd` context, so this is
    /// just "nothing to do," logged and exited rather than blocking on
    /// input that will never come.
    NothingToSnapshot,
}

/// Decides the mode purely from whether any sources are configured and
/// which command was requested — no I/O, no rendering, so every branch
/// is deterministically testable.
pub fn determine_mode(sources_is_empty: bool, requested: RequestedCommand) -> Mode {
    match (requested, sources_is_empty) {
        (RequestedCommand::Sources, _) => Mode::Sources,
        (RequestedCommand::Snapshot, true) => Mode::NothingToSnapshot,
        (RequestedCommand::Snapshot, false) => Mode::SnapshotHeadless,
        (RequestedCommand::Default, true) => Mode::FirstRunSources,
        (RequestedCommand::Default, false) => Mode::Dashboard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_command_with_no_sources_goes_to_first_run_sources() {
        assert_eq!(
            determine_mode(true, RequestedCommand::Default),
            Mode::FirstRunSources
        );
    }

    #[test]
    fn default_command_with_sources_goes_to_dashboard() {
        assert_eq!(
            determine_mode(false, RequestedCommand::Default),
            Mode::Dashboard
        );
    }

    #[test]
    fn sources_command_always_goes_to_sources_screen() {
        assert_eq!(
            determine_mode(true, RequestedCommand::Sources),
            Mode::Sources
        );
        assert_eq!(
            determine_mode(false, RequestedCommand::Sources),
            Mode::Sources
        );
    }

    #[test]
    fn snapshot_command_with_no_sources_has_nothing_to_do() {
        assert_eq!(
            determine_mode(true, RequestedCommand::Snapshot),
            Mode::NothingToSnapshot
        );
    }

    #[test]
    fn snapshot_command_with_sources_runs_headless() {
        assert_eq!(
            determine_mode(false, RequestedCommand::Snapshot),
            Mode::SnapshotHeadless
        );
    }
}
