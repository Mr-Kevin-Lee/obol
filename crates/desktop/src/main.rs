// Hides the console window on Windows release builds. Harmless on
// macOS, which is the only target today (spec §4) — kept because it's
// the one line that's painful to remember to add later.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    obol_desktop::run()
}
