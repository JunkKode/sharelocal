// The Windows release build is a GUI app, so no console window appears.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sharelocal::run();
}
