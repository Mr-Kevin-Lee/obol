//! The Tauri desktop frontend (spec §6.1, §15, §16 v0.3, D43) — the
//! GUI that replaces the `ratatui` TUI, sitting on the same `obol-core`
//! as the CLI does. No domain logic lives here: this crate assembles
//! core's output into a shape the webview can render and nothing more,
//! which is what keeps D43's "no logic rewrite" honest.

pub mod view;

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running obol desktop app");
}
