#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ahead_app::app;

pub fn main() {
    if ahead_proxy::run_if_requested() {
        return;
    }

    ahead_app::shell_environment::initialize_login_shell_path();
    app::launch();
}
