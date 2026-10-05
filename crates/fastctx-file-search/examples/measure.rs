//! Measures a real index without opening any result file.
use fastctx_file_search::Search;
use fastctx_types::file_search::FileSearchResult;
use std::time::Instant;

fn main() -> std::io::Result<()> {
    let roots = std::env::args().skip(1).collect::<Vec<_>>();
    assert!(!roots.is_empty(), "supply absolute roots");
    let started = Instant::now();
    let search = Search::new(roots)?;
    println!("start_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    loop {
        if let FileSearchResult::Matches { ready: true, total, .. } = search.query(String::new(), 0)? {
            println!("indexed={total} cold_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    for text in ["", "m", "ma", "main", "main.rs", "world", "src/lib", "模型", "!literal$"] {
        let mut samples = Vec::new();
        let mut counts = (0, 0);
        // Twenty samples give a five-percent empirical percentile step.
        for _ in 0..20 {
            let now = Instant::now();
            let result = search.query(text.into(), 20)?;
            samples.push(now.elapsed().as_secs_f64() * 1000.0);
            if let FileSearchResult::Matches { ready, total, files, .. } = result {
                assert!(ready, "index changed during measurement");
                counts = (total, files.len());
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "query={text:?} matched={} returned={} median_ms={:.3} p95_ms={:.3} max_ms={:.3}",
            counts.0, counts.1, samples[10], samples[18], samples[19]
        );
    }
    let now = Instant::now();
    drop(search);
    println!("close_ms={:.3}", now.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
