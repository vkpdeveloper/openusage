// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if openusage_lib::handle_cli_from_env() {
        return;
    }

    openusage_lib::run()
}
