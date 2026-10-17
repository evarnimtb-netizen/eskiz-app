// Release builds on Windows are GUI apps (no console window); debug builds keep the console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    eskiz_app::run();
}
