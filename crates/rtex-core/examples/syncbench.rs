//! In-process SyncTeX timing: `cargo run --release --example syncbench -- doc.pdf doc.tex`
use rtex_core::synctex;
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (pdf, src) = (Path::new(&args[1]), std::fs::canonicalize(&args[2]).unwrap());
    let t = Instant::now();
    let s = synctex::cached(pdf).unwrap();
    let parse = t.elapsed();
    let n = 10_000;
    let t = Instant::now();
    let mut hits = 0;
    for i in 0..n {
        let page = 1 + (i % s.pages.len()) as u32;
        if s.inverse(page, 100.0 + (i % 400) as f64, 80.0 + (i * 7 % 640) as f64).is_some() {
            hits += 1;
        }
    }
    let inv = t.elapsed() / n as u32;
    let t = Instant::now();
    for i in 0..n {
        let _ = s.forward(&src, 1 + (i % 40) as u32, None);
    }
    let fwd = t.elapsed() / n as u32;
    let t = Instant::now();
    let _ = synctex::cached(pdf).unwrap();
    let cached = t.elapsed();
    println!("parse {:?} | cached lookup {:?} | inverse {:?}/query ({hits} hits) | forward {:?}/query", parse, cached, inv, fwd);
}
