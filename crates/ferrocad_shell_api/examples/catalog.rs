//! Print the built-in command catalogue as the JSON the Python registry reads.
//!
//! ```sh
//! cargo run -p ferrocad_shell_api --example catalog > python/ferrocad_shell/catalog.json
//! ```
//!
//! `ferrocad_shell_api::catalog`'s test fails when the checked-in file is stale, so
//! this is only the way to *update* it.

fn main() {
    print!("{}", ferrocad_shell_api::catalog::json_text());
}
