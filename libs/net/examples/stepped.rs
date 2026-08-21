//! Exercise the stepped fetch against a real server.
//!
//! `cargo run --release -p nyx-net --example stepped -- https://en.wikipedia.org/wiki/Unix`
//!
//! The thing actually under test is whether a TLS stream survives being walked away from: `Fetch`
//! uses a short socket timeout and treats the resulting error as "not yet", so every poll may
//! interrupt rustls part-way through a record or even a handshake. If that were unsound it would
//! show up here as a stall or a decrypt failure rather than as a mystery on hardware.

use nyx_net::{Fetch, Progress};

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://example.com".to_string());
    let mut fetch = Fetch::new(&url).expect("url");
    let start = std::time::Instant::now();
    let mut polls = 0usize;
    let mut connecting = 0usize;

    loop {
        polls += 1;
        match fetch.poll() {
            Progress::Connecting(phase) => {
                connecting += 1;
                println!("  poll {polls:>4}  {} {}", phase.label(), fetch.url().host);
            }
            Progress::Receiving { got, total } => {
                if polls % 10 == 0 {
                    println!("  poll {polls:>4}  {got:>8} / {total:?}");
                }
            }
            Progress::Done(resp) => {
                println!(
                    "done in {} polls ({connecting} pre-body), {:.0} ms\n  status {}  {} bytes  {}",
                    polls,
                    start.elapsed().as_secs_f64() * 1000.0,
                    resp.status,
                    resp.body.len(),
                    resp.url
                );
                // Prove the body actually decoded rather than merely arriving.
                let text = resp.text();
                println!("  <title> present: {}", text.contains("<title"));
                println!("  head: {:?}", &text.chars().take(60).collect::<String>());
                return;
            }
            Progress::Failed(e) => {
                println!("failed after {polls} polls: {e}");
                std::process::exit(1);
            }
        }
        if polls > 20_000 {
            println!("gave up: {polls} polls without finishing");
            std::process::exit(2);
        }
    }
}
