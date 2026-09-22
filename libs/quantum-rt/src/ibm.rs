//! IBM Quantum, as a Nyx backend.
//!
//! ## Why bother, when IonQ already works
//!
//! **Free time on real quantum hardware.** IBM's Open plan gives 10 minutes per 28-day window on
//! their production QPUs. IonQ's `simulator` is free but is a simulator; its `qpu.*` targets are
//! metered. A Bell circuit at 1024 shots is a few seconds of QPU time, so 10 minutes a month is a
//! lot of real measurements.
//!
//! That is the entire justification, and it is a good one — but IBM is substantially harder than
//! IonQ, in two places.
//!
//! ## Difficulty 1: authentication is a two-step exchange
//!
//! IonQ wants one header. IBM wants:
//!
//! 1. `POST https://iam.cloud.ibm.com/identity/token`, **form-encoded** (not JSON), exchanging the
//!    API key for a bearer token that **expires in 3600 seconds**.
//! 2. Every subsequent request carries `Authorization: Bearer <token>` **and**
//!    `Service-CRN: <instance CRN>` — a ~120-character identifier that is a second credential.
//!
//! The CRN length is a large part of why `docs/quantum/remote.md` recommends baking credentials into
//! the image: Nyx has no clipboard and cannot express a paste chord, so typing a CRN by hand is not
//! a workflow anyone would use twice.
//!
//! ## ★★ Difficulty 2: IBM requires ISA circuits, and that is a compiler
//!
//! IonQ accepts abstract gates and transpiles server-side. IBM does not: the Qiskit Runtime
//! primitives require an **ISA circuit** — already decomposed to the backend's basis gates *and*
//! already mapped onto its physical qubits, submitted as OpenQASM 3.
//!
//! Heron's basis set is **`cz`, `id`, `rz`, `sx`, `x`** on a **heavy-hex** lattice where a qubit has
//! at most three neighbours. So a general solution needs two passes: decompose every gate into that
//! basis, then *route* logical qubits onto the lattice, inserting SWAPs where a two-qubit gate spans
//! non-adjacent physical qubits. Routing is a real algorithm, it is bug-prone, and its quality
//! directly affects result fidelity — a bad route adds gates, and every gate adds error.
//!
//! **This module deliberately does not do routing.** It handles the circuits that need none:
//!
//! * **Decomposition** is fixed arithmetic — `h` → `rz(π/2)·sx·rz(π/2)`, `cx(a,b)` → `h(b)·cz·h(b)`.
//! * **Placement** picks a *connected path* of physical qubits from the backend's coupling map, so
//!   every two-qubit gate in the circuit already lands on an edge.
//!
//! That covers Bell pairs and GHZ chains — the demonstration circuits — and refuses everything else
//! **by name**. A circuit this cannot place is rejected, never silently mis-compiled: a wrong
//! placement would still return a perfectly plausible histogram of a different computation, which is
//! exactly the failure mode this subsystem exists to prevent.
//!
//! ## ⚠️ What was and was not verified
//!
//! Read from IBM's live documentation on 2026-09-12: the IAM exchange, the `Authorization` +
//! `Service-CRN` header pair, the `POST /api/v1/jobs` body shape
//! (`program_id`/`backend`/`params.pubs`), that a PUB is `[circuit, params, shots]` with the circuit
//! as an OpenQASM 3 **string**, that ISA circuits are required, and Heron's basis gates.
//!
//! **Not** verified: the exact endpoint for a backend's coupling map, and the precise shape of the
//! results document. Both are handled defensively — [`IbmProvider::coupling_map`] refuses rather
//! than assuming an edge exists, and [`decode_samples`] reports an unrecognised result shape instead
//! of guessing. The first real submission should be checked against the job's own record.

use crate::provider::{clock_looks_sane, require_https, Credentials, ProviderTarget, QuantumProvider};
use nyx_json::{obj, parse, s as jstr, Value};
use nyx_quantum::backend::{
    JobHandle, JobState, Outcome, Provenance, QpuBackend, Readout, Shots, Stage,
};
use nyx_quantum::circuit::{Circuit, Gate, Op};
use nyx_quantum::device::{ExecModel, QpuInfo, QpuStatus, QuantumDeviceKind, Topology};
use nyx_quantum::QpuError;
use std::string::{String, ToString};
use std::vec::Vec;

/// IAM token exchange.
pub const IAM_URL: &str = "https://iam.cloud.ibm.com/identity/token";
/// Qiskit Runtime API root.
pub const API_ROOT: &str = "https://quantum.cloud.ibm.com/api/v1";

/// π to `f64` precision, for the fixed rotation angles below.
const PI: f64 = core::f64::consts::PI;

/// Gates this module can express in Heron's basis set.
///
/// ★ Anything absent is refused by name. `rz`/`sx`/`x` are basis gates and pass through; `h`, `z`,
/// `y`, `s`, `sdg`, `t`, `tdg` have exact fixed decompositions; `cx` becomes `h·cz·h`.
///
/// `swap` and `ccx` are deliberately **not** here even though both are expressible: `swap` is three
/// CNOTs and `ccx` is six, and both would make circuits whose two-qubit gates no longer sit on a
/// single edge — which is precisely the case placement cannot handle without routing.
fn is_supported(g: Gate) -> bool {
    matches!(
        g,
        Gate::H
            | Gate::X
            | Gate::Y
            | Gate::Z
            | Gate::S
            | Gate::Sdg
            | Gate::T
            | Gate::Tdg
            | Gate::Rz(_)
            | Gate::Cx
    )
}

fn supported_gate_mask() -> u64 {
    let mut m = 0u64;
    for g in Gate::ALL {
        if is_supported(g) {
            m |= 1 << g.bit();
        }
    }
    m
}

/// Append the basis-gate expansion of a single-qubit gate acting on physical qubit `q`.
///
/// ⚠️ Emits **OpenQASM 2.0** syntax — `rz(θ) q[5];`, not `rz(θ) $5;`.
///
/// This was QASM 3 until IBM's loader rejected it outright:
///
/// ```text
/// Error loading QASM circuit with the standard Qiskit loaders:
/// '<input>:1,9: can only handle OpenQASM 2.0, but given 3
/// ```
///
/// QASM 2 has **no hardware-qubit notation** (`$0`) at all, so a transpiled circuit follows the
/// Qiskit convention instead: declare `qreg q[<full device width>]` and index it by *physical*
/// qubit number. The register spanning the whole device is what marks the indices as physical.
///
/// Every formula here is exact up to a global phase, which is unobservable in the measurement
/// statistics this backend returns.
///
/// * `rz(θ)` is a basis gate — emitted directly.
/// * `sx` is a basis gate; `x` is too.
/// * `h  = rz(π/2)·sx·rz(π/2)`
/// * `s  = rz(π/2)`, `sdg = rz(-π/2)`, `t = rz(π/4)`, `tdg = rz(-π/4)`
/// * `z  = rz(π)`
/// * `y  = rz(π)·x`  (Y = i·X·Z, and the phase is global)
fn emit_single(out: &mut String, g: Gate, q: u32) -> Result<(), QpuError> {
    let mut rz = |a: f64, out: &mut String| {
        out.push_str(&std::format!("rz({}) q[{}];\n", fmt_angle(a), q));
    };
    match g {
        Gate::Rz(a) => rz(a, out),
        Gate::X => out.push_str(&std::format!("x q[{}];\n", q)),
        Gate::Z => rz(PI, out),
        Gate::S => rz(PI / 2.0, out),
        Gate::Sdg => rz(-PI / 2.0, out),
        Gate::T => rz(PI / 4.0, out),
        Gate::Tdg => rz(-PI / 4.0, out),
        Gate::H => {
            rz(PI / 2.0, out);
            out.push_str(&std::format!("sx q[{}];\n", q));
            rz(PI / 2.0, out);
        }
        Gate::Y => {
            rz(PI, out);
            out.push_str(&std::format!("x q[{}];\n", q));
        }
        other => return Err(QpuError::UnsupportedGate(other.qasm_name())),
    }
    Ok(())
}

/// Format an angle with enough precision to round-trip, and without an exponent.
///
/// OpenQASM 3 accepts a plain decimal; `1e-17` would be legal but is needlessly hostile to read in a
/// log, and these angles are all small multiples of π/4.
fn fmt_angle(a: f64) -> String {
    let mut s = std::format!("{:.12}", a);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.push('0');
    }
    s
}

/// Turn a Nyx circuit into an ISA OpenQASM 3 program on the given physical qubits.
///
/// `placement[i]` is the physical qubit that logical qubit `i` runs on. Every two-qubit gate must
/// land on an edge of the device — [`place_on_coupling_map`] is what guarantees that, and
/// [`IbmProvider::begin`] refuses if it cannot.
///
/// ⚠️ The emitted program addresses **hardware qubits** (`$0`, `$1`, …), which is what makes it an
/// ISA circuit. A program using virtual `q[0]` registers would be transpiled by the service — or,
/// since that path is deprecated, rejected.
pub fn emit_isa(c: &Circuit, placement: &[u32], device_width: u32) -> Result<String, QpuError> {
    if placement.len() < c.qubits as usize {
        return Err(QpuError::Unsupported(std::format!(
            "placement has {} qubits but the circuit needs {}",
            placement.len(),
            c.qubits
        )));
    }
    // Every physical qubit used must fit inside the declared register, or the loader rejects the
    // program with an out-of-range index.
    let highest = placement.iter().copied().max().unwrap_or(0);
    if highest >= device_width {
        return Err(QpuError::Unsupported(std::format!(
            "placement uses physical qubit {highest} but the device declares {device_width}"
        )));
    }

    let measured = c.measured_qubits();
    let mut body = String::new();

    for op in &c.ops {
        match op {
            // Measurements are emitted at the end, in wire order, so the classical register index
            // matches Nyx's label order rather than the order they appear in the program.
            Op::Measure { .. } => {}
            Op::Reset { .. } => {
                return Err(QpuError::Unsupported(
                    "mid-circuit reset is not emitted by this backend; \
                     remove it or run on the local simulator"
                        .to_string(),
                ))
            }
            Op::Gate { gate, qubits } => {
                if !is_supported(*gate) {
                    return Err(QpuError::UnsupportedGate(gate.qasm_name()));
                }
                match gate {
                    Gate::Cx => {
                        let (ctrl, targ) = (placement[qubits[0] as usize], placement[qubits[1] as usize]);
                        // CX(a,b) = H(b) · CZ(a,b) · H(b), with H itself expanded to the basis.
                        emit_single(&mut body, Gate::H, targ)?;
                        body.push_str(&std::format!("cz q[{}],q[{}];\n", ctrl, targ));
                        emit_single(&mut body, Gate::H, targ)?;
                    }
                    g => emit_single(&mut body, *g, placement[qubits[0] as usize])?,
                }
            }
        }
    }

    // ⚠️ `qelib1.inc`, not `stdgates.inc` — the latter is the QASM 3 standard library and does not
    // exist in QASM 2. `rz`, `x` and `cz` are all defined in qelib1; `sx` is not, but Qiskit's
    // loader recognises it as one of its legacy custom instructions, which is how every transpiled
    // IBM circuit expresses the basis.
    let mut out = String::from("OPENQASM 2.0;\ninclude \"qelib1.inc\";\n");
    // The register spans the WHOLE device: that is what makes the indices physical qubits rather
    // than virtual ones the service would feel free to re-map.
    out.push_str(&std::format!("qreg q[{}];\n", device_width));
    out.push_str(&std::format!("creg c[{}];\n", measured.len().max(1)));
    out.push_str(&body);
    for (i, &lq) in measured.iter().enumerate() {
        // QASM 2 measurement is an arrow, not an assignment.
        out.push_str(&std::format!("measure q[{}] -> c[{}];\n", placement[lq as usize], i));
    }
    Ok(out)
}

/// The Qiskit Runtime job payload for one circuit.
///
/// ## ★★ `version: 2` is mandatory and its absence is silent
///
/// Without it the runtime treats the submission as a **V1** primitive and rejects it outright:
///
/// ```text
/// Failed to execute program: 'The V1 Primitives are not supported.
/// Please use Primitives V2'   (error 1515)
/// ```
///
/// The trap is that `params.pubs` is *already* the V2 input shape, so a payload missing this field
/// looks entirely correct — the version is carried separately, and its absence selects V1.
///
/// A **PUB** ("primitive unified bloc") is `(circuit, parameter_values, shots)`. Nyx sends one
/// circuit with no bound parameters.
pub fn submission_body(qasm: &str, backend: &str, shots: u32) -> Value {
    obj(&[
        ("program_id", jstr("sampler")),
        ("backend", jstr(backend)),
        (
            "params",
            obj(&[
                (
                    "pubs",
                    Value::Array(std::vec![Value::Array(std::vec![
                        jstr(qasm),
                        Value::Null,
                        Value::Number(shots as f64),
                    ])]),
                ),
                ("options", obj(&[])),
                ("version", Value::Number(2.0)),
            ]),
        ),
    ])
}

/// Map logical qubits onto a connected path of physical qubits.
///
/// ★ This is what stands in for a router. It finds a path in the coupling graph such that logical
/// qubit `i` and `i+1` are always physically adjacent, then checks that **every** two-qubit gate in
/// the circuit acts on a logically adjacent pair.
///
/// That is exactly the class Bell pairs and GHZ chains fall into. Anything else returns `None` and
/// the caller refuses — rather than placing it anyway and returning a plausible histogram of a
/// different computation.
pub fn place_on_coupling_map(c: &Circuit, edges: &[(u32, u32)]) -> Option<Vec<u32>> {
    let n = c.qubits as usize;
    if n == 0 {
        return Some(Vec::new());
    }
    if n == 1 {
        // A single-qubit circuit can go anywhere a qubit exists.
        let q = edges.first().map(|e| e.0).unwrap_or(0);
        return Some(std::vec![q]);
    }

    // Every two-qubit gate must join logically adjacent qubits, or a path placement cannot satisfy
    // it and we would need real routing.
    for op in &c.ops {
        if let Op::Gate { gate, qubits } = op {
            if gate.arity() == 2 {
                let (a, b) = (qubits[0] as i32, qubits[1] as i32);
                if (a - b).abs() != 1 {
                    return None;
                }
            }
            if gate.arity() > 2 {
                return None;
            }
        }
    }

    // Undirected adjacency.
    let mut adj: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut push = |x: u32, y: u32, adj: &mut Vec<(u32, Vec<u32>)>| {
        match adj.iter_mut().find(|(k, _)| *k == x) {
            Some(e) => {
                if !e.1.contains(&y) {
                    e.1.push(y)
                }
            }
            None => adj.push((x, std::vec![y])),
        }
    };
    for &(a, b) in edges {
        push(a, b, &mut adj);
        push(b, a, &mut adj);
    }

    // Depth-first search for a simple path of length n. The lattice is tiny relative to n (2 or 3),
    // so an exhaustive walk from each start is instant and needs no heuristic.
    fn walk(
        adj: &[(u32, Vec<u32>)],
        path: &mut Vec<u32>,
        n: usize,
    ) -> bool {
        if path.len() == n {
            return true;
        }
        let last = *path.last().unwrap();
        let Some((_, nbrs)) = adj.iter().find(|(k, _)| *k == last) else { return false };
        for &nb in nbrs {
            if path.contains(&nb) {
                continue;
            }
            path.push(nb);
            if walk(adj, path, n) {
                return true;
            }
            path.pop();
        }
        false
    }

    let mut starts: Vec<u32> = adj.iter().map(|(k, _)| *k).collect();
    starts.sort_unstable();
    for s in starts {
        let mut path = std::vec![s];
        if walk(&adj, &mut path, n) {
            return Some(path);
        }
    }
    None
}

/// Read the shots out of a Sampler V2 result document.
///
/// ⚠️ Unlike IonQ, IBM returns **per-shot samples**, not probabilities — so this produces real
/// counts. The samples are hex strings of the classical register.
///
/// The exact document shape was not verified against a live job, so this refuses rather than
/// guessing when it cannot find the samples array.
pub fn decode_samples(v: &Value, num_bits: usize) -> Result<Vec<(String, u64)>, QpuError> {
    // `results[0].data.<register>.samples`. The register is named `c` because that is what
    // `emit_isa` declares (`creg c[n]`) — but the key is the register's name, not a fixed word, so
    // a differently-named register is looked up rather than treated as a missing result.
    let data = v
        .path(&["results"])
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("data"))
        .ok_or_else(|| {
            QpuError::Provider(
                "could not find results[0].data in the provider's response".to_string(),
            )
        })?;

    let samples = data
        .path(&["c", "samples"])
        .and_then(|s| s.as_array())
        .or_else(|| {
            // Fall back to the first register that carries a samples array.
            data.as_object()?
                .iter()
                .find_map(|(_, reg)| reg.path(&["samples"]).and_then(|s| s.as_array()))
        })
        .ok_or_else(|| {
            QpuError::Provider(
                "results[0].data carried no register with a `samples` array".to_string(),
            )
        })?;

    let mut counts: Vec<(String, u64)> = Vec::new();
    for s in samples {
        let text = s
            .as_str()
            .ok_or_else(|| QpuError::Provider("a sample was not a string".to_string()))?;
        let hex = text.strip_prefix("0x").unwrap_or(text);
        let value = u64::from_str_radix(hex, 16)
            .map_err(|_| QpuError::Provider(std::format!("sample {text:?} is not hexadecimal")))?;

        // ★ Same little-endian convention as IonQ: bit i of the register is classical bit i, and
        // classical bit i was written by the i-th measured wire in ascending wire order. Nyx labels
        // wire 0 leftmost.
        let mut label = String::with_capacity(num_bits);
        for i in 0..num_bits {
            label.push(if (value >> i) & 1 == 1 { '1' } else { '0' });
        }

        match counts.iter_mut().find(|(l, _)| *l == label) {
            Some(e) => e.1 += 1,
            None => counts.push((label, 1)),
        }
    }
    Ok(counts)
}

/// Whether a backend name denotes a simulator.
///
/// ⚠️ Conservative, like IonQ's: anything not clearly a physical device is treated as a simulator, so
/// an unrecognised name **claims less, not more**. IBM's QPUs are named `ibm_<site>`; their managed
/// simulators carry `simulator` or `_stabilizer`/`_statevector` in the name.
pub fn target_is_simulator(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("simulator") || n.contains("stabilizer") || n.contains("statevector") || n.contains("fake") {
        return true;
    }
    !n.starts_with("ibm_")
}

/// One entry in the account's job history.
///
/// Enough to choose between jobs and nothing more — the id is what any action actually needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSummary {
    pub id: String,
    /// `"Completed"`, `"Failed"`, … as the provider spells it. Not normalised: a status Nyx does not
    /// recognise should still be shown, not hidden behind a guess.
    pub status: String,
    pub backend: String,
    /// Creation timestamp as the provider formats it, or empty.
    pub created: String,
}

impl JobSummary {
    /// Whether this job has a result worth fetching.
    pub fn is_complete(&self) -> bool {
        self.status.eq_ignore_ascii_case("completed")
    }
}

/// The IBM Quantum backend.
pub struct IbmProvider {
    api_key: String,
    crn: String,
    bearer: Option<String>,
    backend_name: String,
    target: ProviderTarget,
    next: u64,
    jobs: Vec<IbmJob>,
}

struct IbmJob {
    handle: JobHandle,
    id: Option<String>,
    qasm: String,
    shots: u32,
    measured: Vec<u8>,
    done: bool,
    /// Consecutive transient failures while polling. Reset on any successful call.
    transient: u32,
}

/// How many consecutive network/auth failures to absorb while watching a submitted job.
///
/// ★ This exists because the first version got it badly wrong: **any** error while polling marked
/// the job finished and reported failure. On a lossy link that turns one dropped packet into a dead
/// job — and the job is not actually dead. IBM has it, IBM is running it, and on a metered backend
/// IBM is billing for it. We would simply have stopped watching.
///
/// So transport errors and 401s are retried (a 401 is often just the hour-old bearer token, which
/// `call` clears so the next attempt re-authenticates). Only an explicit `Failed`/`Cancelled` from
/// the provider, or a malformed result, is terminal.
/// ★ Raised 6 → 40 (2026-09-21) after a real job was abandoned on a lossy link.
///
/// Measured: submission succeeded (job `daokr58pqrnc739a5go0` was accepted), then six consecutive
/// connect timeouts ended the watch — roughly twenty seconds of tolerance on a link showing
/// `badsig=41`. The job was *fine*; we simply stopped looking.
///
/// This is no longer the real bound. `RemoteJob`'s overall time budget in the terminal is, and it is
/// measured in minutes. This only exists to stop an unbounded hammer on a hard failure.
const MAX_TRANSIENT: u32 = 40;

/// Whether an error is worth retrying rather than abandoning the job for.
fn is_transient(e: &QpuError) -> bool {
    matches!(e, QpuError::Provider(_) | QpuError::Auth(_) | QpuError::Unreachable(_))
}

/// Translate a transport failure, preserving whether anything reached the wire.
///
/// ★ The distinction is the whole point: [`QpuError::Unreachable`] means the request provably never
/// left this machine, which is what makes re-sending a **POST** safe. Anything else might have been
/// received and answered, and replaying it would create a second job.
fn transport_err(ctx: &str, e: nyx_net::Error) -> QpuError {
    if e.is_before_send() {
        QpuError::Unreachable(std::format!("{ctx}: {e}"))
    } else {
        QpuError::Provider(std::format!("{ctx}: {e}"))
    }
}

impl IbmProvider {
    /// Open a session against one backend.
    pub fn open(target: ProviderTarget, now_unix: u64) -> Result<IbmProvider, QpuError> {
        clock_looks_sane(now_unix)?;
        require_https(API_ROOT)?;
        require_https(IAM_URL)?;

        let creds = Credentials::load();
        let api_key = creds.get("ibm.token").map(|s| s.to_string()).ok_or_else(|| {
            QpuError::Auth(
                "no IBM API key: add `ibm.token = …` to quantum-credentials.txt and rebuild"
                    .to_string(),
            )
        })?;
        // ⚠️ The CRN is a SECOND required credential, ~120 characters. Named separately in the error
        // because "authentication failed" with a valid key and no CRN is otherwise baffling.
        let crn = creds.get("ibm.crn").map(|s| s.to_string()).ok_or_else(|| {
            QpuError::Auth(
                "no IBM instance CRN: add `ibm.crn = crn:v1:bluemix:…` to quantum-credentials.txt. \
                 IBM needs both a key and a CRN"
                    .to_string(),
            )
        })?;

        Ok(IbmProvider {
            api_key,
            crn,
            bearer: None,
            backend_name: target.name.clone(),
            target,
            next: 1,
            jobs: Vec::new(),
        })
    }

    pub fn device_info(&self, id: u32) -> QpuInfo {
        let mut i = QpuInfo {
            id,
            kind: QuantumDeviceKind::Qpu as u32,
            status: QpuStatus::Remote as u32,
            remote_is_simulator: u32::from(self.target.is_simulator),
            qubits: self.target.qubits,
            // ⚠️ Heavy-hex: a qubit has at most three neighbours. Reporting all-to-all here would be
            // a lie that matters — it is the property that decides whether a circuit needs routing.
            topology: Topology::Grid as u32,
            exec_model: ExecModel::Batch as u32,
            gate_set: supported_gate_mask(),
            max_shots: 0,
            max_depth: 0,
            queue_depth: self.target.queue.unwrap_or(u32::MAX),
            queue_capacity: 0,
            coherence_t1_ns: 0,
            coherence_t2_ns: 0,
            calibrated_unix: 0,
            ..QpuInfo::default()
        };
        i.set_vendor("IBM");
        i.set_arch(if self.target.is_simulator { "cloud-simulator" } else { "superconducting" });
        i.set_name(&self.target.name);
        i
    }

    /// Exchange the API key for a bearer token.
    ///
    /// ⚠️ **Form-encoded, not JSON** — the one place in this subsystem that is. The token expires in
    /// an hour; for a single job that is far longer than we need, so there is no refresh loop. A job
    /// queued for over an hour will fail authentication on its next poll, and the error says so
    /// rather than looking like a revoked key.
    fn authenticate(&mut self) -> Result<(), QpuError> {
        if self.bearer.is_some() {
            return Ok(());
        }
        let url = nyx_net::Url::parse(IAM_URL)
            .map_err(|e| QpuError::Provider(std::format!("bad IAM URL: {e}")))?;
        let body = std::format!(
            "grant_type=urn:ibm:params:oauth:grant-type:apikey&apikey={}",
            urlencode(&self.api_key)
        );
        let req = nyx_net::Request::post(body.into_bytes(), "application/x-www-form-urlencoded")
            .header("Accept", "application/json");

        let resp = nyx_net::request_once_within(&url, &req, core::time::Duration::from_secs(20))
            .map_err(|e| transport_err("IAM", e))?;
        if !(200..300).contains(&resp.status) {
            // The body can echo the key; report only the status.
            return Err(QpuError::Auth(std::format!(
                "IBM rejected the API key during token exchange (HTTP {})",
                resp.status
            )));
        }
        let text = String::from_utf8(resp.body)
            .map_err(|_| QpuError::Provider("IAM response was not UTF-8".to_string()))?;
        let doc = parse(&text)
            .map_err(|e| QpuError::Provider(std::format!("malformed IAM response: {e}")))?;
        let tok = doc
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| QpuError::Auth("IAM returned no access_token".to_string()))?;
        self.bearer = Some(tok.to_string());
        Ok(())
    }

    /// One authenticated request against the Runtime API.
    ///
    /// ⚠️ **Blocks** for one HTTPS round trip. See [`IbmProvider::poll`].
    fn call(&mut self, post: bool, path: &str, body: Vec<u8>) -> Result<Value, QpuError> {
        self.authenticate()?;
        let bearer = self.bearer.clone().unwrap_or_default();

        let url_text = std::format!("{API_ROOT}{path}");
        require_https(&url_text)?;
        let url = nyx_net::Url::parse(&url_text)
            .map_err(|e| QpuError::Provider(std::format!("bad URL: {e}")))?;

        let req = if post {
            nyx_net::Request::post(body, "application/json")
        } else {
            nyx_net::Request::get()
        }
        .header("Authorization", &std::format!("Bearer {bearer}"))
        .header("Service-CRN", &self.crn)
        .header("Accept", "application/json");

        // ⚠️ 20 s, not `nyx_net`'s 60 s default. This is a POLLING path: the caller asks again every
        // few seconds, so a fast "that attempt failed" beats a long stall — and a 60-second stall
        // inside one poll is indistinguishable, from the outside, from the job merely taking a
        // while. Measured: a `watch` sat silent for 110 s, which was two stalled polls rather than
        // anything happening.
        let resp = nyx_net::request_once_within(&url, &req, core::time::Duration::from_secs(20))
            .map_err(|e| transport_err("runtime", e))?;

        match resp.status {
            200..=299 => {}
            401 | 403 => {
                // The bearer may simply have aged out; clear it so a retry re-authenticates.
                self.bearer = None;
                return Err(QpuError::Auth(
                    "IBM rejected the request (401/403). the bearer token may have expired \
                     after an hour, or the CRN does not match the key"
                        .to_string(),
                ));
            }
            429 => {
                return Err(QpuError::Provider(
                    "IBM is rate-limiting this account (429; the limit is 5 jobs/minute)".to_string(),
                ))
            }
            s => return Err(QpuError::Provider(std::format!("IBM returned HTTP {s}"))),
        }

        let text = String::from_utf8(resp.body)
            .map_err(|_| QpuError::Provider("the response was not UTF-8".to_string()))?;
        parse(&text).map_err(|e| QpuError::Provider(std::format!("malformed JSON from IBM: {e}")))
    }

    /// Recent jobs on this account, newest first.
    ///
    /// ⚠️ The response shape was not verified against a live account, so the parse is defensive:
    /// the array is looked for under several plausible keys, and each entry contributes whatever
    /// fields it actually has rather than being skipped for missing one. A job with no readable id
    /// is dropped, because there is nothing useful to do with it.
    pub fn list_jobs(&mut self, limit: usize) -> Result<Vec<JobSummary>, QpuError> {
        let doc = self.call(false, &std::format!("/jobs?limit={limit}"), Vec::new())?;

        let arr = doc
            .get("jobs")
            .and_then(|v| v.as_array())
            .or_else(|| doc.get("data").and_then(|v| v.as_array()))
            .or_else(|| doc.as_array())
            .ok_or_else(|| {
                QpuError::Provider("the job list was not an array Nyx recognises".to_string())
            })?;

        let mut out = Vec::new();
        for j in arr {
            let Some(id) = j.get("id").and_then(|v| v.as_str()) else { continue };
            out.push(JobSummary {
                id: id.to_string(),
                status: status_of(j).to_string(),
                backend: j
                    .get("backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("—")
                    .to_string(),
                created: j
                    .get("created")
                    .and_then(|v| v.as_str())
                    .or_else(|| j.get("created_time").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string(),
            });
        }
        Ok(out)
    }

    /// Re-attach to a job that was already submitted, by its provider id.
    ///
    /// ★ Exists because a job outlives the connection that created it. A flaky link can lose contact
    /// after submission, and the job then keeps running on IBM's hardware and keeps consuming the
    /// runtime allowance — so its result has already been paid for. Without this, that result is
    /// unreachable and the money is simply gone.
    ///
    /// ⚠️ `measured` and `shots` cannot be recovered from the provider, so the caller supplies what
    /// it originally submitted. Getting `measured` wrong would mislabel the basis states: the
    /// histogram would be real but its column headings would not match the circuit.
    pub fn attach(&mut self, job_id: &str, measured: Vec<u8>, shots: u32) -> JobHandle {
        let h = JobHandle(self.next);
        self.next += 1;
        self.jobs.push(IbmJob {
            handle: h,
            id: Some(job_id.to_string()),
            // Never re-submitted — `poll` skips straight to the status stage because `id` is set.
            qasm: String::new(),
            shots,
            measured,
            done: false,
            transient: 0,
        });
        h
    }

    /// Decide whether a polling error ends the job or is worth another go.
    ///
    /// ★ The rule: a failure to *reach* IBM says nothing about the job, which is running on their
    /// hardware regardless of what this Wi-Fi link is doing. Giving up would abandon a result we may
    /// already have been billed for.
    ///
    /// When we do finally give up, the message says the job may still be running — because it may,
    /// and a user who thinks it died will not go and look for it.
    fn absorb(&mut self, i: usize, e: QpuError, stage: Stage) -> JobState {
        if !is_transient(&e) {
            self.jobs[i].done = true;
            return JobState::Failed(e);
        }
        self.jobs[i].transient += 1;
        if self.jobs[i].transient <= MAX_TRANSIENT {
            return JobState::Running(stage);
        }
        let id = self.jobs[i].id.clone().unwrap_or_default();
        self.jobs[i].done = true;
        JobState::Failed(QpuError::Provider(std::format!(
            "lost contact with IBM after {MAX_TRANSIENT} attempts ({e}). \
             job {id} may still be running on their hardware — check your IBM Quantum dashboard"
        )))
    }

    /// The backend's coupling map, as undirected edges.
    ///
    /// ⚠️ The endpoint shape was not verified against a live account, so a failure here is reported
    /// rather than substituted with an assumption. Guessing that qubits 0 and 1 are adjacent is true
    /// on every IBM lattice I know of — and "true on every one I know of" is exactly the reasoning
    /// this subsystem refuses to act on for something that decides where a computation runs.
    fn coupling_map(&mut self) -> Result<(Vec<(u32, u32)>, u32), QpuError> {
        let name = self.backend_name.clone();
        let doc = self.call(false, &std::format!("/backends/{name}/configuration"), Vec::new())?;
        let raw = doc
            .get("coupling_map")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                QpuError::Provider(std::format!(
                    "{name} reported no coupling_map, so Nyx cannot place a circuit on it safely"
                ))
            })?;

        let mut edges = Vec::new();
        for pair in raw {
            let Some(p) = pair.as_array() else { continue };
            if p.len() != 2 {
                continue;
            }
            let (Some(a), Some(b)) = (p[0].as_u64(), p[1].as_u64()) else { continue };
            edges.push((a as u32, b as u32));
        }
        if edges.is_empty() {
            return Err(QpuError::Provider(std::format!(
                "{name}'s coupling map was empty or unparseable"
            )));
        }

        // The device's full width, needed for the `qreg q[N]` declaration. Prefer what the backend
        // states; fall back to the highest index the coupling map mentions, which is a lower bound
        // and still produces a valid program.
        let width = doc
            .get("n_qubits")
            .and_then(|v| v.as_u64())
            .or_else(|| doc.get("num_qubits").and_then(|v| v.as_u64()))
            .map(|v| v as u32)
            .unwrap_or_else(|| edges.iter().map(|(a, b)| *a.max(b)).max().unwrap_or(0) + 1);

        Ok((edges, width))
    }
}

/// Percent-encode a value for `application/x-www-form-urlencoded`.
///
/// IBM API keys are base64url-ish and would usually survive unencoded, but `+` in a form body means
/// a space — an unencoded `+` in a key silently becomes a different key and produces an
/// authentication failure with no clue as to why.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&std::format!("%{:02X}", b)),
        }
    }
    out
}

impl QuantumProvider for IbmProvider {
    fn name(&self) -> &str {
        "ibm"
    }

    fn targets(&mut self) -> Result<Vec<ProviderTarget>, QpuError> {
        Ok(std::vec![self.target.clone()])
    }
}

impl QpuBackend for IbmProvider {
    fn name(&self) -> &str {
        "ibm"
    }

    fn describe(&self) -> &str {
        if self.target.is_simulator {
            "IBM Quantum, simulator backend - REMOTE but still a classical simulation"
        } else {
            "IBM Quantum, superconducting QPU - real hardware, 10 free minutes/month on the Open plan"
        }
    }

    fn info(&self) -> QpuInfo {
        self.device_info(0)
    }

    fn begin(&mut self, c: &Circuit, shots: Shots, _seed: u64) -> Result<JobHandle, QpuError> {
        self.validate(c, shots)?;

        // Placement first: a circuit that cannot be placed must be refused BEFORE a job is created,
        // and certainly before anything is billed.
        let (edges, width) = self.coupling_map()?;
        let placement = place_on_coupling_map(c, &edges).ok_or_else(|| {
            QpuError::Unsupported(
                "this circuit needs SWAP routing to fit IBM's heavy-hex lattice, and Nyx has no \
                 router. Bell pairs and GHZ chains work; arbitrary circuits do not"
                    .to_string(),
            )
        })?;
        let qasm = emit_isa(c, &placement, width)?;

        let h = JobHandle(self.next);
        self.next += 1;
        self.jobs.push(IbmJob {
            handle: h,
            id: None,
            qasm,
            shots: shots.0,
            measured: c.measured_qubits(),
            done: false,
            transient: 0,
        });
        Ok(h)
    }

    /// Advance the job by one HTTPS round trip.
    ///
    /// ⚠️ **Blocks** for the duration of one request. Safe from a terminal command that has announced
    /// it will block; must **not** be pumped from `apps/shell`, which is the window server.
    fn poll(&mut self, h: JobHandle) -> JobState {
        let Some(i) = self.jobs.iter().position(|j| j.handle == h) else {
            return JobState::Failed(QpuError::NoSuchJob(h));
        };
        if self.jobs[i].done {
            return JobState::Failed(QpuError::NoSuchJob(h));
        }

        // Stage 1: submit.
        if self.jobs[i].id.is_none() {
            let body =
                submission_body(&self.jobs[i].qasm, &self.backend_name, self.jobs[i].shots)
                    .to_string()
                    .into_bytes();

            return match self.call(true, "/jobs", body) {
                // ★ Retry ONLY when the request provably never left this machine. A lossy link
                // makes DNS and connect failures common, and abandoning the run for one dropped
                // packet — before anything was even submitted — is needless.
                //
                // ⚠️ Any other error might mean IBM received the job and we lost the reply. Replaying
                // a POST then would queue the SAME circuit twice and bill twice, so those stay fatal.
                Err(e @ QpuError::Unreachable(_)) => self.absorb(i, e, Stage::Submitting),
                Err(e) => {
                    self.jobs[i].done = true;
                    JobState::Failed(e)
                }
                Ok(v) => match v.get("id").and_then(|x| x.as_str()) {
                    None => {
                        self.jobs[i].done = true;
                        JobState::Failed(QpuError::Provider(
                            "IBM accepted the job but returned no id".to_string(),
                        ))
                    }
                    Some(id) => {
                        self.jobs[i].id = Some(id.to_string());
                        JobState::Running(Stage::Queued(None))
                    }
                },
            };
        }

        let id = self.jobs[i].id.clone().unwrap();

        // Stage 2: status.
        //
        // ⚠️ A transport failure here does NOT end the job — see `MAX_TRANSIENT`. The job is running
        // on IBM's hardware whatever this link is doing, and abandoning it would stop us collecting
        // a result we have already paid for.
        let doc = match self.call(false, &std::format!("/jobs/{id}"), Vec::new()) {
            Ok(v) => {
                self.jobs[i].transient = 0;
                v
            }
            Err(e) => return self.absorb(i, e, Stage::Queued(None)),
        };
        let status = status_of(&doc);
        match stage_for(status, failure_reason(&doc)) {
            // An explicit Failed/Cancelled from the provider IS terminal — that is the job's own
            // verdict, not a network opinion about it.
            Err(e) => {
                self.jobs[i].done = true;
                // Name the job so it can be looked up on the dashboard, which carries the full
                // record including anything this response omitted.
                return JobState::Failed(match e {
                    QpuError::Provider(m) => {
                        QpuError::Provider(std::format!("{m} [job {id}]"))
                    }
                    other => other,
                });
            }
            Ok(Some(st)) => return JobState::Running(st),
            Ok(None) => {}
        }

        // Stage 3: results. Same treatment — the answer exists on IBM's side now, so a failure to
        // fetch it is worth retrying rather than discarding.
        let results = match self.call(false, &std::format!("/jobs/{id}/results"), Vec::new()) {
            Ok(v) => {
                self.jobs[i].transient = 0;
                v
            }
            Err(e) => return self.absorb(i, e, Stage::Fetching),
        };

        let measured = self.jobs[i].measured.clone();
        let shots = self.jobs[i].shots;
        let info = self.info();
        self.jobs[i].done = true;

        match decode_samples(&results, measured.len()) {
            Err(e) => JobState::Failed(e),
            Ok(counts) => JobState::Done(std::boxed::Box::new(Outcome {
                // ★ IBM returns per-shot samples, so these really are counts — unlike IonQ, which
                // returns probabilities and cannot be converted to counts honestly.
                readout: Readout::Counts(counts),
                shots_requested: shots,
                measured,
                provenance: Provenance::from_info("ibm", &info),
            })),
        }
    }

    fn cancel(&mut self, h: JobHandle) {
        if let Some(i) = self.jobs.iter().position(|j| j.handle == h) {
            if let Some(id) = self.jobs[i].id.clone() {
                let _ = self.call(true, &std::format!("/jobs/{id}/cancel"), Vec::new());
            }
            self.jobs.remove(i);
        }
    }

    fn status_note(&self, h: JobHandle) -> Option<String> {
        let j = self.jobs.iter().find(|j| j.handle == h)?;
        // Silence while things are going well; speak up the moment they are not.
        (j.transient > 0)
            .then(|| std::format!("link failing, retry {}/{}", j.transient, MAX_TRANSIENT))
    }
}

/// Map IBM's job status to a stage. `None` means finished successfully.
///
/// `reason` is IBM's own explanation, when the document carries one — see [`failure_reason`].
fn stage_for(status: &str, reason: Option<String>) -> Result<Option<Stage>, QpuError> {
    match status {
        "Completed" | "completed" => Ok(None),
        "Queued" | "queued" => Ok(Some(Stage::Queued(None))),
        "Running" | "running" | "In progress" | "Validating" => Ok(Some(Stage::Running)),
        "Cancelled" | "cancelled" | "Canceled" => Err(QpuError::Cancelled),
        "Failed" | "failed" | "Error" => Err(QpuError::Provider(match reason {
            // ★ Print IBM's reason verbatim. "the job failed" tells a user nothing they can act on,
            // and the reason is the difference between a malformed circuit, an unavailable backend,
            // and an exhausted allowance — three problems with three different fixes.
            Some(r) => std::format!("IBM rejected the job: {r}"),
            None => "IBM reported the job failed, and gave no reason in the job record"
                .to_string(),
        })),
        // ⚠️ Unknown is treated as still-running, never as success: assuming completion would fetch
        // results that do not exist.
        _ => Ok(Some(Stage::Running)),
    }
}

/// Dig IBM's own failure explanation out of a job document.
///
/// The field has moved between API versions and differs between a rejected circuit and a platform
/// error, so several shapes are tried rather than one guessed at. Returns `None` if the document
/// genuinely carries no explanation — which is itself worth reporting, rather than papering over
/// with an invented message.
fn failure_reason(doc: &Value) -> Option<String> {
    const PATHS: &[&[&str]] = &[
        &["state", "reason"],
        &["state", "reasonCode"],
        &["reason"],
        &["error", "message"],
        &["errors", "message"],
        &["message"],
        &["failure_reason"],
    ];
    for p in PATHS {
        let Some(v) = doc.path(p) else { continue };
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(n) = v.as_f64() {
            return Some(std::format!("code {n}"));
        }
    }
    // Some responses carry the detail in an array of error objects.
    if let Some(arr) = doc.get("errors").and_then(|v| v.as_array()) {
        for e in arr {
            if let Some(m) = e.get("message").and_then(|v| v.as_str()) {
                if !m.is_empty() {
                    return Some(m.to_string());
                }
            }
        }
    }
    None
}

/// The job's status, wherever this API version keeps it.
///
/// Top level in some responses, nested under `state` in others. Checking both is cheaper than
/// finding out on hardware that an unrecognised shape made every job look like it was still running.
fn status_of(doc: &Value) -> &str {
    doc.get("status")
        .and_then(|v| v.as_str())
        .or_else(|| doc.path(&["state", "status"]).and_then(|v| v.as_str()))
        .unwrap_or("Unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the ISA emitter ─────────────────────────────────────────────────────────────────────────

    /// ★★ The regression guard for IBM error 1515:
    ///
    /// ```text
    /// Failed to execute program: 'The V1 Primitives are not supported. Please use Primitives V2'
    /// ```
    ///
    /// ⚠️ The payload *looked* right without it — `pubs` is already the V2 shape. The version is
    /// carried separately and its absence silently selects V1.
    #[test]
    fn the_payload_declares_primitives_v2() {
        let body = submission_body("OPENQASM 2.0;\n", "ibm_fez", 1024);
        assert_eq!(body.path(&["params", "version"]).and_then(|v| v.as_u64()), Some(2));
        assert_eq!(body.get("program_id").and_then(|v| v.as_str()), Some("sampler"));
        assert_eq!(body.get("backend").and_then(|v| v.as_str()), Some("ibm_fez"));

        // A PUB is (circuit, parameter_values, shots).
        let pub0 = body
            .path(&["params", "pubs"])
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|p| p.as_array())
            .expect("pubs[0]");
        assert_eq!(pub0.len(), 3);
        assert!(pub0[0].as_str().unwrap().starts_with("OPENQASM 2.0;"));
        assert!(pub0[1].is_null(), "no bound parameters");
        assert_eq!(pub0[2].as_u64(), Some(1024));

        // Integers must not serialise as "1024.0" — several APIs reject that where they want an int.
        let text = body.to_string();
        assert!(text.contains(r#""version":2"#), "{text}");
        assert!(text.contains("1024"), "{text}");
        assert!(!text.contains("1024.0"), "{text}");
    }

    /// ★★ The regression guard for the failure IBM actually reported:
    ///
    /// ```text
    /// can only handle OpenQASM 2.0, but given 3
    /// ```
    #[test]
    fn it_emits_openqasm_2_not_3() {
        let qasm = emit_isa(&Circuit::bell(), &[0, 1], 156).unwrap();
        assert!(qasm.starts_with("OPENQASM 2.0;\n"), "{qasm}");
        // `stdgates.inc` is the QASM 3 standard library and does not exist in QASM 2.
        assert!(qasm.contains("include \"qelib1.inc\";"), "{qasm}");
        assert!(!qasm.contains("stdgates"), "{qasm}");
        // QASM 2 has no `bit[]` declaration and no `$N` hardware-qubit notation.
        assert!(!qasm.contains("bit["), "{qasm}");
        assert!(!qasm.contains('$'), "QASM 2 has no hardware-qubit syntax:\n{qasm}");
    }

    #[test]
    fn a_bell_circuit_emits_only_basis_gates() {
        let qasm = emit_isa(&Circuit::bell(), &[0, 1], 156).unwrap();
        // ★ The whole point of ISA: only cz/rz/sx/x/measure may appear.
        for line in qasm.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with("OPENQASM")
                || line.starts_with("include")
                || line.starts_with("qreg")
                || line.starts_with("creg")
                || line.starts_with("measure")
            {
                continue;
            }
            let head: String = line.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            assert!(
                matches!(head.as_str(), "cz" | "rz" | "sx" | "x"),
                "non-basis gate {head:?} in ISA output:\n{qasm}"
            );
        }
    }

    /// ⚠️ QASM 2 cannot say "hardware qubit". The convention is a register spanning the WHOLE
    /// device, indexed by physical qubit — that is what marks the indices as physical rather than
    /// virtual ones the service would feel free to re-map.
    #[test]
    fn the_register_spans_the_device_and_is_indexed_physically() {
        let qasm = emit_isa(&Circuit::bell(), &[13, 14], 156).unwrap();
        assert!(qasm.contains("qreg q[156];"), "{qasm}");
        assert!(qasm.contains("creg c[2];"), "{qasm}");
        assert!(qasm.contains("cz q[13],q[14];"), "{qasm}");
        assert!(qasm.contains("measure q[13] -> c[0];"), "{qasm}");
        assert!(qasm.contains("measure q[14] -> c[1];"), "{qasm}");
    }

    #[test]
    fn a_placement_outside_the_device_is_refused() {
        // Would otherwise emit q[200] against qreg q[156] and be rejected by the loader with an
        // out-of-range index — better caught here, where the message can say what happened.
        match emit_isa(&Circuit::bell(), &[200, 201], 156) {
            Err(QpuError::Unsupported(why)) => {
                // Names the highest offending index and the declared width.
                assert!(why.contains("201"), "{why}");
                assert!(why.contains("156"), "{why}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn h_expands_to_the_documented_decomposition() {
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).measure(0, 0);
        let qasm = emit_isa(&c, &[5], 156).unwrap();
        // h = rz(pi/2) sx rz(pi/2)
        let gates: Vec<&str> = qasm
            .lines()
            .filter(|l| l.starts_with("rz") || l.starts_with("sx"))
            .collect();
        assert_eq!(gates.len(), 3, "{qasm}");
        assert!(gates[0].starts_with("rz(1.570796326795)"), "{gates:?}");
        assert_eq!(gates[1], "sx q[5];");
        assert!(gates[2].starts_with("rz(1.570796326795)"), "{gates:?}");
    }

    #[test]
    fn cx_becomes_h_cz_h_on_the_target() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::Cx, &[0, 1]).measure_all();
        let qasm = emit_isa(&c, &[2, 3], 156).unwrap();
        let cz_at = qasm.find("cz q[2],q[3];").expect("no cz");
        // Three basis gates for H before the cz, and three after — all on the TARGET (q[3]).
        let before = &qasm[..cz_at];
        let after = &qasm[cz_at..];
        assert_eq!(before.matches("q[3]").count(), 3, "H on target before cz:\n{qasm}");
        assert!(after.contains("sx q[3];"), "H on target after cz:\n{qasm}");
        // The control must never receive a single-qubit gate in this decomposition.
        assert!(!before.contains("sx q[2];"), "{qasm}");
    }

    #[test]
    fn phase_gates_become_rz_with_the_right_angle() {
        for (g, want) in [
            (Gate::Z, "rz(3.14159265359)"),
            (Gate::S, "rz(1.570796326795)"),
            (Gate::Sdg, "rz(-1.570796326795)"),
            (Gate::T, "rz(0.785398163397)"),
            (Gate::Tdg, "rz(-0.785398163397)"),
        ] {
            let mut c = Circuit::new(1, 1);
            c.gate(g, &[0]).measure(0, 0);
            let qasm = emit_isa(&c, &[0], 156).unwrap();
            assert!(qasm.contains(want), "{:?} -> {qasm}", g);
        }
    }

    #[test]
    fn measurements_are_emitted_in_wire_order_at_the_end() {
        let mut c = Circuit::new(2, 2);
        // Program order is deliberately reversed; the emitted order must not be.
        c.gate(Gate::H, &[0]).measure(1, 1).measure(0, 0);
        let qasm = emit_isa(&c, &[7, 8], 156).unwrap();
        let i0 = qasm.find("measure q[7] -> c[0];").expect("wire 0");
        let i1 = qasm.find("measure q[8] -> c[1];").expect("wire 1");
        assert!(i0 < i1, "classical bit order must follow wire order:\n{qasm}");
    }

    #[test]
    fn unsupported_gates_are_refused_by_name() {
        for g in [Gate::Swap, Gate::Ccx, Gate::Rx(0.5), Gate::Ry(0.5), Gate::U3(1.0, 2.0, 3.0)] {
            let arity = g.arity();
            let mut c = Circuit::new(3, 3);
            let qs: Vec<u8> = (0..arity as u8).collect();
            c.gate(g, &qs);
            match emit_isa(&c, &[0, 1, 2], 156) {
                Err(QpuError::UnsupportedGate(name)) => assert_eq!(name, g.qasm_name()),
                other => panic!("{:?} should be refused by name, got {other:?}", g),
            }
        }
    }

    #[test]
    fn reset_is_refused_with_a_way_out() {
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).reset(0).measure(0, 0);
        match emit_isa(&c, &[0], 156) {
            Err(QpuError::Unsupported(why)) => assert!(why.contains("local simulator"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    // ── placement ───────────────────────────────────────────────────────────────────────────────

    /// A fragment of a heavy-hex lattice: a chain with one degree-3 branch.
    fn lattice() -> Vec<(u32, u32)> {
        std::vec![(0, 1), (1, 2), (2, 3), (3, 4), (1, 14), (14, 18)]
    }

    #[test]
    fn a_bell_pair_lands_on_a_real_edge() {
        let p = place_on_coupling_map(&Circuit::bell(), &lattice()).expect("placeable");
        assert_eq!(p.len(), 2);
        let e = lattice();
        assert!(
            e.contains(&(p[0], p[1])) || e.contains(&(p[1], p[0])),
            "placement {p:?} is not an edge"
        );
    }

    #[test]
    fn a_ghz_chain_lands_on_a_connected_path() {
        for n in 2..=5u8 {
            let p = place_on_coupling_map(&Circuit::ghz(n), &lattice())
                .unwrap_or_else(|| panic!("ghz({n}) should be placeable on a chain"));
            assert_eq!(p.len(), n as usize);
            let e = lattice();
            for w in p.windows(2) {
                assert!(
                    e.contains(&(w[0], w[1])) || e.contains(&(w[1], w[0])),
                    "ghz({n}) placement {p:?} has a non-adjacent step"
                );
            }
        }
    }

    /// ★ The refusal that matters. A circuit needing routing must be rejected, not placed anyway —
    /// a wrong placement returns a plausible histogram of a different computation.
    #[test]
    fn a_circuit_needing_routing_is_refused() {
        // CX between logical 0 and 2 — not adjacent in any path placement.
        let mut c = Circuit::new(3, 3);
        c.gate(Gate::H, &[0]).gate(Gate::Cx, &[0, 2]).measure_all();
        assert!(place_on_coupling_map(&c, &lattice()).is_none());

        // A three-qubit gate likewise.
        let mut c = Circuit::new(3, 3);
        c.gate(Gate::Ccx, &[0, 1, 2]).measure_all();
        assert!(place_on_coupling_map(&c, &lattice()).is_none());
    }

    #[test]
    fn a_chain_longer_than_the_lattice_is_refused() {
        // 7 qubits on a 7-node fragment whose longest simple path is shorter.
        let small = std::vec![(0, 1), (1, 2)];
        assert!(place_on_coupling_map(&Circuit::ghz(4), &small).is_none());
        assert!(place_on_coupling_map(&Circuit::ghz(3), &small).is_some());
    }

    #[test]
    fn placement_uses_the_branch_when_the_straight_chain_is_too_short() {
        // Only a branch gives a path of 3: 2-1-14.
        let branchy = std::vec![(1, 2), (1, 14)];
        let p = place_on_coupling_map(&Circuit::ghz(3), &branchy).expect("placeable via the branch");
        assert_eq!(p.len(), 3);
        assert_eq!(p[1], 1, "the degree-2 node must be the middle of the path: {p:?}");
    }

    // ── results ─────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn samples_are_counted_into_a_histogram() {
        let doc = parse(
            r#"{"results":[{"data":{"c":{"samples":["0x0","0x3","0x3","0x0","0x3"],"num_bits":2}}}]}"#,
        )
        .unwrap();
        let counts = decode_samples(&doc, 2).unwrap();
        let get = |l: &str| counts.iter().find(|(k, _)| k == l).map(|(_, v)| *v).unwrap_or(0);
        assert_eq!(get("00"), 2);
        assert_eq!(get("11"), 3);
        assert_eq!(counts.iter().map(|(_, n)| n).sum::<u64>(), 5);
    }

    /// ★★ Bell and GHZ samples are palindromes and cannot catch a bit-order error. This can.
    #[test]
    fn an_asymmetric_sample_proves_the_bit_order() {
        // 0x1 = bit 0 set = classical bit 0 = the FIRST measured wire. Nyx labels wire 0 leftmost.
        let doc = parse(r#"{"results":[{"data":{"c":{"samples":["0x1"],"num_bits":2}}}]}"#).unwrap();
        assert_eq!(decode_samples(&doc, 2).unwrap()[0].0, "10");

        let doc = parse(r#"{"results":[{"data":{"c":{"samples":["0x2"],"num_bits":2}}}]}"#).unwrap();
        assert_eq!(decode_samples(&doc, 2).unwrap()[0].0, "01");

        let doc = parse(r#"{"results":[{"data":{"c":{"samples":["0x4"],"num_bits":3}}}]}"#).unwrap();
        assert_eq!(decode_samples(&doc, 3).unwrap()[0].0, "001");
    }

    /// The samples live under the classical register's *name*. `emit_isa` declares `creg c[n]`, so
    /// it is normally `c` — but looking up the register rather than hardcoding the word means a
    /// differently-named one still reads.
    #[test]
    fn samples_are_found_under_a_differently_named_register() {
        let doc =
            parse(r#"{"results":[{"data":{"meas":{"samples":["0x3","0x0"],"num_bits":2}}}]}"#)
                .unwrap();
        let counts = decode_samples(&doc, 2).unwrap();
        assert_eq!(counts.iter().map(|(_, n)| n).sum::<u64>(), 2);
    }

    #[test]
    fn an_unrecognised_result_shape_is_reported_not_guessed() {
        assert!(decode_samples(&parse("{}").unwrap(), 2).is_err());
        assert!(decode_samples(&parse(r#"{"results":[]}"#).unwrap(), 2).is_err());
        assert!(
            decode_samples(&parse(r#"{"results":[{"data":{"c":{"samples":["zz"]}}}]}"#).unwrap(), 2)
                .is_err(),
            "a non-hex sample must be an error, not a silent zero"
        );
    }

    // ── identity and honesty ────────────────────────────────────────────────────────────────────

    #[test]
    fn only_ibm_prefixed_names_count_as_hardware() {
        assert!(!target_is_simulator("ibm_fez"));
        assert!(!target_is_simulator("ibm_brisbane"));
        assert!(!target_is_simulator("ibm_torino"));

        assert!(target_is_simulator("simulator_statevector"));
        assert!(target_is_simulator("ibmq_qasm_simulator"));
        assert!(target_is_simulator("fake_sherbrooke"));
        // Conservative on anything unrecognised.
        assert!(target_is_simulator("something-new"));
        assert!(target_is_simulator(""));
    }

    fn provider(name: &str, sim: bool, qubits: u32) -> IbmProvider {
        IbmProvider {
            api_key: "k".to_string(),
            crn: "crn:v1:bluemix:public:quantum-computing:us-east:a/x::".to_string(),
            bearer: None,
            backend_name: name.to_string(),
            target: ProviderTarget {
                name: name.to_string(),
                qubits,
                is_simulator: sim,
                queue: None,
            },
            next: 1,
            jobs: Vec::new(),
        }
    }

    #[test]
    fn a_real_qpu_reports_hardware_and_invents_no_metrics() {
        let i = provider("ibm_fez", false, 156).info();
        assert_eq!(i.status_label(), "REMOTE/hw");
        assert!(i.is_quantum());
        assert_eq!(i.arch_str(), "superconducting");
        // ⚠️ Heavy-hex, max degree 3. Claiming all-to-all would be the lie that decides whether a
        // circuit needs routing.
        assert_eq!(i.topology(), Topology::Grid);
        assert_eq!(i.coherence_t1(), None);
        assert_eq!(i.calibrated(), None);
    }

    #[test]
    fn a_cloud_simulator_is_labelled_as_one() {
        let i = provider("ibmq_qasm_simulator", true, 32).info();
        assert_eq!(i.status_label(), "REMOTE/sim");
        assert!(!i.is_quantum());
        assert!(Provenance::from_info("ibm", &i).disclosure().contains("SIMULATOR"));
    }

    #[test]
    fn the_advertised_gate_set_matches_what_can_be_emitted() {
        let mask = supported_gate_mask();
        for g in Gate::ALL {
            assert_eq!(
                nyx_quantum::circuit::gate_set_contains(mask, g),
                is_supported(g),
                "{} advertised inconsistently",
                g.qasm_name()
            );
        }
        // Swap and Ccx are expressible in principle but excluded, because they break placement.
        assert!(!is_supported(Gate::Swap));
        assert!(!is_supported(Gate::Ccx));
    }

    #[test]
    fn status_strings_map_to_stages_and_unknown_keeps_waiting() {
        assert!(matches!(stage_for("Completed", None), Ok(None)));
        assert!(matches!(stage_for("Queued", None), Ok(Some(Stage::Queued(None)))));
        assert!(matches!(stage_for("Running", None), Ok(Some(Stage::Running))));
        assert!(matches!(stage_for("Cancelled", None), Err(QpuError::Cancelled)));
        assert!(matches!(stage_for("Failed", None), Err(QpuError::Provider(_))));
        assert!(matches!(stage_for("Validating", None), Ok(Some(Stage::Running))));
    }

    /// ★ "the job failed" is not an error message. The reason is the difference between a malformed
    /// circuit, an unavailable backend and an exhausted allowance — three fixes, not one.
    #[test]
    fn ibms_own_failure_reason_is_surfaced_verbatim() {
        let doc = parse(
            r#"{"status":"Failed","state":{"status":"Failed","reason":"Instruction cz is not supported"}}"#,
        )
        .unwrap();
        assert_eq!(status_of(&doc), "Failed");
        match stage_for(status_of(&doc), failure_reason(&doc)) {
            Err(QpuError::Provider(m)) => assert!(m.contains("Instruction cz is not supported"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reason_is_found_wherever_the_api_version_keeps_it() {
        let cases = [
            r#"{"reason":"top level"}"#,
            r#"{"error":{"message":"nested error"}}"#,
            r#"{"errors":[{"message":"array form"}]}"#,
            r#"{"message":"plain message"}"#,
        ];
        for c in cases {
            assert!(failure_reason(&parse(c).unwrap()).is_some(), "no reason found in {c}");
        }
        // A document with genuinely no explanation reports that, rather than inventing one.
        assert!(failure_reason(&parse(r#"{"status":"Failed"}"#).unwrap()).is_none());
        match stage_for("Failed", None) {
            Err(QpuError::Provider(m)) => assert!(m.contains("gave no reason"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// The status lives at the top level in some responses and under `state` in others. Reading only
    /// one would make every job look like it was still running.
    #[test]
    fn status_is_found_at_either_nesting_level() {
        assert_eq!(status_of(&parse(r#"{"status":"Running"}"#).unwrap()), "Running");
        assert_eq!(status_of(&parse(r#"{"state":{"status":"Completed"}}"#).unwrap()), "Completed");
        assert_eq!(status_of(&parse(r#"{}"#).unwrap()), "Unknown");
    }

    /// ★★ The bug this guards was live: **any** polling error marked the job finished, so one
    /// dropped packet on a lossy link killed a job that IBM was still running — and still billing.
    #[test]
    fn a_network_blip_while_polling_does_not_abandon_a_running_job() {
        let mut p = provider("ibm_fez", false, 156);
        p.jobs.push(IbmJob {
            handle: JobHandle(1),
            id: Some("job-abc".to_string()),
            qasm: String::new(),
            shots: 1024,
            measured: std::vec![0, 1],
            done: false,
            transient: 0,
        });

        // Transport failures are absorbed; the job stays alive and keeps reporting its stage.
        for _ in 0..MAX_TRANSIENT {
            let st = p.absorb(0, QpuError::Provider("connection timed out".to_string()), Stage::Queued(None));
            assert!(matches!(st, JobState::Running(_)), "a blip must not end the job");
            assert!(!p.jobs[0].done);
        }

        // Past the budget it gives up — and says the job may still be running, because it may.
        match p.absorb(0, QpuError::Provider("connection timed out".to_string()), Stage::Queued(None)) {
            JobState::Failed(QpuError::Provider(m)) => {
                assert!(m.contains("job-abc"), "the message must name the job: {m}");
                assert!(m.contains("may still be running"), "{m}");
            }
            other => panic!("expected an explained give-up, got {other:?}"),
        }
        assert!(p.jobs[0].done);
    }

    /// An expired bearer token surfaces as a 401 and is retryable — `call` clears the token so the
    /// next attempt re-authenticates. A job queued for over an hour hits exactly this.
    #[test]
    fn an_expired_token_is_retried_rather_than_failing_the_job() {
        assert!(is_transient(&QpuError::Auth("401".to_string())));
        assert!(is_transient(&QpuError::Provider("timed out".to_string())));

        // The provider's own verdict on the job is NOT a network opinion and must end it.
        assert!(!is_transient(&QpuError::Cancelled));
        assert!(!is_transient(&QpuError::Unsupported("x".to_string())));
        assert!(!is_transient(&QpuError::UnsupportedGate("ccx")));
    }

    #[test]
    fn a_provider_verdict_ends_the_job_immediately() {
        let mut p = provider("ibm_fez", false, 156);
        p.jobs.push(IbmJob {
            handle: JobHandle(1),
            id: Some("job-abc".to_string()),
            qasm: String::new(),
            shots: 8,
            measured: std::vec![0],
            done: false,
            transient: 0,
        });
        assert!(matches!(
            p.absorb(0, QpuError::Cancelled, Stage::Running),
            JobState::Failed(QpuError::Cancelled)
        ));
        assert!(p.jobs[0].done, "a cancelled job must not be retried");
    }

    /// ★★ The idempotency rule, and the reason `Unreachable` exists as a separate variant.
    ///
    /// A POST may be replayed **only** when it provably never reached the wire. Anything that might
    /// have been received and answered must stay fatal, or a lost reply becomes a second queued job
    /// and a second charge on metered hardware.
    #[test]
    fn only_a_provably_unsent_request_may_be_replayed() {
        use nyx_net::Error as NetErr;

        // Never sent: safe to retry.
        assert!(NetErr::Dns { host: "iam.cloud.ibm.com".into() }.is_before_send());
        assert!(NetErr::Connect {
            addr: "1.2.3.4:443".into(),
            source: std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
        }
        .is_before_send());

        // ⚠️ Might have been sent and answered: must NOT be replayed.
        assert!(!NetErr::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "x")).is_before_send());
        assert!(!NetErr::Protocol("garbage".into()).is_before_send());

        // And the mapping preserves that distinction.
        assert!(matches!(
            transport_err("IAM", NetErr::Dns { host: "x".into() }),
            QpuError::Unreachable(_)
        ));
        assert!(matches!(
            transport_err("runtime", NetErr::Protocol("x".into())),
            QpuError::Provider(_)
        ));
    }

    #[test]
    fn an_unreachable_provider_is_retried_like_any_other_transient() {
        assert!(is_transient(&QpuError::Unreachable("dns".to_string())));
    }

    /// ★ A job outlives the connection that created it. Re-attaching must NOT re-submit — that
    /// would queue a second circuit and bill twice for one result.
    #[test]
    fn attaching_to_an_existing_job_never_resubmits() {
        let mut p = provider("ibm_fez", false, 156);
        let h = p.attach("daokr58pqrnc739a5go0", std::vec![0, 1], 1024);

        let job = p.jobs.iter().find(|j| j.handle == h).expect("attached job");
        // `id` already set is what makes `poll` skip the submit stage entirely.
        assert_eq!(job.id.as_deref(), Some("daokr58pqrnc739a5go0"));
        assert!(job.qasm.is_empty(), "an attached job has no circuit to send");
        assert_eq!(job.measured, std::vec![0, 1]);
        assert_eq!(job.shots, 1024);
        assert!(!job.done);
    }

    #[test]
    fn form_encoding_protects_a_key_containing_plus_or_slash() {
        // ⚠️ In a form body a bare `+` means a space — an unencoded key silently becomes a different
        // key, and the failure looks like a revoked credential.
        assert_eq!(urlencode("ab+cd/ef=gh"), "ab%2Bcd%2Fef%3Dgh");
        assert_eq!(urlencode("plain-Key_123.~"), "plain-Key_123.~");
    }
}
