//! `nyx-net` — the HTTP/1.1 + TLS 1.3 transport under Nyx's browser work.
//!
//! Sits on `std::net::TcpStream` (the PAL in `vendor/nyx-std/sys/net_nyx.rs`, which is real on Nyx)
//! and rustls with a pure-Rust crypto provider, because `ring`/`aws-lc` are C/asm and there is no C
//! toolchain for this target.
//!
//! Deliberately portable: nothing here is nyx-specific except `rng`, which is `#[cfg]`-gated. That
//! means the URL and HTTP parsing are host-testable with `cargo test`, and only the transport needs
//! hardware — which is the split that keeps the parts most likely to be wrong cheap to check.
//!
//! ```no_run
//! let page = nyx_net::get("https://example.com")?;
//! println!("{} {} bytes", page.status, page.body.len());
//! # Ok::<(), nyx_net::Error>(())
//! ```

mod body;
pub mod dns;
pub mod fetch;
pub mod http;
mod rng;
pub mod url;

pub use fetch::{Fetch, Progress};
// `Request` / `request_once` are the POST path, added for the quantum subsystem's cloud providers.
// ⚠️ `request_once` never retries — see its docs; replaying a POST could submit a job twice.
pub use http::{
    get, get_once, request_once, request_once_within, Error, Request, Response,
};
pub use url::{Scheme, Url};
