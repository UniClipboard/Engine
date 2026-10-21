use std::env;
use std::fs;
use std::io::Write;
use std::time::{Duration, Instant};

fn memory(key: &str) -> u64 {
    fs::read_to_string("/proc/self/status").unwrap().lines()
        .find_map(|line| line.strip_prefix(key).map(|s| s.split_whitespace().next().unwrap().parse().unwrap())).unwrap()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    let workers: u32 = args[1].parse()?;
    let iterations: usize = args[3].parse()?;
    let data = fs::read(&args[2])?;
    std::thread::sleep(Duration::from_millis(100));
    let baseline = memory("VmRSS:");
    let mut times = Vec::new();
    let mut output_len = 0;
    for _ in 0..iterations {
        let started = Instant::now();
        let compressed = if workers == 0 {
            zstd::bulk::compress(&data, 3)?
        } else {
            let mut encoder = zstd::Encoder::new(Vec::new(), 3)?;
            encoder.multithread(workers)?;
            encoder.write_all(&data)?;
            encoder.finish()?
        };
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        output_len = compressed.len();
        assert_eq!(zstd::bulk::decompress(&compressed, data.len())?, data);
    }
    times.sort_by(f64::total_cmp);
    let peak = memory("VmHWM:");
    std::thread::sleep(Duration::from_millis(100));
    let settled = memory("VmRSS:");
    println!("{{\"workers\":{workers},\"input_bytes\":{},\"output_bytes\":{output_len},\"median_ms\":{:.4},\"p95_ms\":{:.4},\"baseline_rss_kib\":{baseline},\"peak_rss_kib\":{peak},\"settled_rss_kib\":{settled},\"iterations\":{iterations}}}", data.len(), times[times.len()/2], times[(times.len()*95/100).min(times.len()-1)]);
    Ok(())
}
