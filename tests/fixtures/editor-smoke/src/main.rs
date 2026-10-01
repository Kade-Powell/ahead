mod math;

fn main() {
    // AHEAD_SEARCH_NEEDLE: search, selection, and Quick Open fixture.
    println!("AHEAD smoke test: {} 🦀", math::double(21));
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_runs_without_network_access() {
        assert_eq!(super::math::double(21), 42);
    }
}
