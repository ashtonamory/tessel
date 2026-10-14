//! Print a `.tessel.json` file in canonical form; problems go to stderr.
//! `cargo run -p doc --example canon -- FILE`

fn main() {
    let path = std::env::args().nth(1).expect("usage: canon FILE");
    let loaded = doc::load_file(std::path::Path::new(&path)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    for p in &loaded.problems {
        eprintln!("problem: {p}");
    }
    print!("{}", doc::save_string(&loaded.doc));
}
