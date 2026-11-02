#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
fn main() -> eframe::Result {
    pcb_clearance::gui::run()
}
