// Use the Windows GUI subsystem in both debug and release builds.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    chachong_desktop_lib::run()
}
