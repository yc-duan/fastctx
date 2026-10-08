fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let started = std::time::Instant::now();
    let window = fastctx_hands::run::window::window(&path).unwrap();
    println!("{:.2}s {} pieces, {} lines", started.elapsed().as_secs_f64(), window.pieces.len(), window.lines);
}
