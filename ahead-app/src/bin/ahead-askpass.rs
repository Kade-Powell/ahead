fn main() {
    if !ahead_proxy::git_askpass::run_if_requested() {
        std::process::exit(1);
    }
}
