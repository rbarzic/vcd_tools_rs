use std::path::PathBuf;

#[path = "../tests/fixtures/fst/generator.rs"]
mod generator;

fn main() {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tests/fixtures/fst/tiny.fst"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture directory");
    }
    generator::write_tiny_fst(&path);
    println!("wrote {}", path.display());
}
