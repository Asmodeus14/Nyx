//! IonQ, as a Nyx quantum backend.
//!
//! ## Why IonQ is the first provider
//!
//! Two reasons, both practical:
//!
//! 1. **Its wire format is nearly Nyx's own IR.** A circuit is a JSON array of
//!    `{"gate":"h","target":0}` objects, so [`serialise_circuit`] is a match arm per gate rather
//!    than a compiler. Compare IBM's Qiskit Runtime, which wants circuits already transpiled to a
//!    device's basis gates.
//! 2. **Authentication is one header.** `Authorization: apiKey <token>`. AWS Braket needs SigV4
//!    request signing, which needs HMAC-SHA256, and `libs/crypto` has only SHA-1 — so Braket is not
//!    reachable from this tree today without new cryptography.
//!
//! ## API version
//!
//! Written against **v0.4**, verified against `docs.ionq.com` on 2026-09-12. That matters because the
//! shape changed from v0.3: the submission key is `type: "ionq.circuit.v1"` (not
//! `format: "ionq.circuit.v0"`), the target field is `backend` (not `target`), the input carries
//! `gateset: "qis"`, and results moved to `/jobs/{id}/results/probabilities`.
//!
//! ⚠️ **What was and was not verified.** The endpoint paths, the submission body shape, the
//! `Authorization` form, and the `h`/`cnot` gate encodings were read from the live documentation.
//! The remaining abstract-gateset gate *names* follow IonQ's published `qis` gate list. They are
//! encoded in exactly one table — [`gate_name`] — and any gate not in it is **refused by name**
//! rather than guessed at, so an unverified spelling cannot silently produce a circuit that means
//! something else. `docs/quantum/remote.md` records this, and the first real submission should be
//! checked against the job's own `gate_counts` field.
//!
//! ## ★★ The histogram is probabilities keyed by a little-endian integer
//!
//! Two things to get right, and both are easy to get wrong in a way that still looks plausible.
//!
//! **Probabilities, not counts.** The response is `{"0": 0.497, "3": 0.503}`. It does not say how
//! many shots produced each bucket. Multiplying by the requested shot count to print "509 of 1024"
//! would invent a measurement nobody made — so this returns [`nyx_quantum::backend::Readout`]`::Probabilities`
//! and the terminal prints probabilities.
//!
//! **Little-endian keys.** The key is an integer whose **bit `i` is qubit `i`**. Nyx's basis-state
//! labels read left to right as wire 0 first, so key `1` on two qubits is `"10"` — not `"01"`.
//! ⚠️ Bell and GHZ states cannot catch this error: their outcomes are all-zeros and all-ones, which
//! are palindromes. [`decode_histogram`] has a deliberately asymmetric test for exactly that reason.

use crate::provider::{clock_looks_sane, require_https, Credentials, ProviderTarget, QuantumProvider};
use nyx_json::{obj, parse, s as jstr, Value};
use nyx_quantum::backend::{JobHandle, JobState, Outcome, Provenance, QpuBackend, Readout, Shots, Stage};
use nyx_quantum::circuit::{Circuit, Gate, Op};
use nyx_quantum::device::{ExecModel, QpuInfo, QpuStatus, QuantumDeviceKind, Topology};
use nyx_quantum::QpuError;
use std::string::{String, ToString};
use std::vec::Vec;

/// API root. v0.4 — see the module docs for what changed from v0.3.
pub const API_ROOT: &str = "https://api.ionq.co/v0.4";

/// The gates Nyx can faithfully serialise for IonQ.
///
/// ★ Anything absent is refused by name. See the module docs: the endpoint shape and the `h`/`cnot`
/// encodings were verified against the live documentation; the rest follow IonQ's published `qis`
/// gateset. A wrong spelling here would produce a circuit that runs and means something else, which
/// is far worse than a refusal, so the set is kept to gates with a single unambiguous name.
fn gate_name(g: Gate) -> Option<&'static str> {
    Some(match g {
        Gate::H => "h",
        Gate::X => "x",
        Gate::Y => "y",
        Gate::Z => "z",
        Gate::S => "s",
        Gate::Sdg => "si",
        Gate::T => "t",
        Gate::Tdg => "ti",
        Gate::Rx(_) => "rx",
        Gate::Ry(_) => "ry",
        Gate::Rz(_) => "rz",
        Gate::Cx => "cnot",
        Gate::Swap => "swap",
        // Toffoli is a cnot with two `controls`. Not emitted because the multi-control field name
        // was not verified, and a three-qubit gate silently encoded as a two-qubit one would be a
        // wrong answer rather than an error.
        Gate::Ccx => return None,
        // U3 has no single abstract-gateset equivalent; it would need decomposing into rotations,
        // which is a transpiler and not a serialiser.
        Gate::U3(..) => return None,
    })
}

/// Everything [`gate_name`] can encode, as a `QpuInfo::gate_set` mask.
fn supported_gate_mask() -> u64 {
    let mut m = 0u64;
    for g in Gate::ALL {
        if gate_name(g).is_some() {
            m |= 1 << g.bit();
        }
    }
    m
}

/// A circuit as IonQ's `qis` gateset JSON.
///
/// Returns the `circuit` array. ⚠️ Measurements are **not** emitted: IonQ measures every qubit at the
/// end of every job and returns a histogram over all of them. There is no per-qubit measurement
/// instruction to send, so an `Op::Measure` contributes nothing to the wire — it only tells Nyx which
/// wires to label. `Op::Reset` has no equivalent at all and is refused.
pub fn serialise_circuit(c: &Circuit) -> Result<Value, QpuError> {
    let mut out: Vec<Value> = Vec::new();

    for op in &c.ops {
        match op {
            Op::Measure { .. } => {}
            Op::Reset { .. } => {
                return Err(QpuError::Unsupported(
                    "IonQ has no mid-circuit reset; remove it or run this on the local simulator"
                        .to_string(),
                ))
            }
            Op::Gate { gate, qubits } => {
                let name = gate_name(*gate).ok_or(QpuError::UnsupportedGate(gate.qasm_name()))?;
                let entry = match gate {
                    Gate::Cx => obj(&[
                        ("gate", jstr(name)),
                        ("control", Value::Number(qubits[0] as f64)),
                        ("target", Value::Number(qubits[1] as f64)),
                    ]),
                    Gate::Swap => obj(&[
                        ("gate", jstr(name)),
                        (
                            "targets",
                            Value::Array(vec![
                                Value::Number(qubits[0] as f64),
                                Value::Number(qubits[1] as f64),
                            ]),
                        ),
                    ]),
                    Gate::Rx(a) | Gate::Ry(a) | Gate::Rz(a) => obj(&[
                        ("gate", jstr(name)),
                        ("target", Value::Number(qubits[0] as f64)),
                        ("rotation", Value::Number(*a)),
                    ]),
                    _ => obj(&[("gate", jstr(name)), ("target", Value::Number(qubits[0] as f64))]),
                };
                out.push(entry);
            }
        }
    }

    Ok(Value::Array(out))
}

/// The full submission body.
pub fn submission_body(c: &Circuit, shots: Shots, backend: &str) -> Result<Value, QpuError> {
    let circuit = serialise_circuit(c)?;
    Ok(obj(&[
        ("type", jstr("ionq.circuit.v1")),
        ("name", jstr("nyx")),
        ("shots", Value::Number(shots.0 as f64)),
        ("backend", jstr(backend)),
        (
            "input",
            obj(&[
                ("qubits", Value::Number(c.qubits as f64)),
                ("gateset", jstr("qis")),
                ("circuit", circuit),
            ]),
        ),
    ]))
}

/// Turn IonQ's probability histogram into Nyx basis-state labels.
///
/// ★★ The endianness conversion. The JSON key is a decimal integer whose **bit `i` is qubit `i`**;
/// a Nyx label reads left to right starting at wire 0. So for `measured = [0, 1]`, key `1` (qubit 0
/// set) becomes `"10"`.
///
/// ⚠️ A Bell or GHZ result cannot detect a mistake here, because all-zeros and all-ones are
/// palindromes. `an_asymmetric_outcome_proves_the_bit_order` is the test that can.
pub fn decode_histogram(v: &Value, measured: &[u8]) -> Result<Vec<(String, f64)>, QpuError> {
    let pairs = v
        .as_object()
        .ok_or_else(|| QpuError::Provider("results were not a JSON object".to_string()))?;

    let m = measured.len();
    let mut out: Vec<(String, f64)> = Vec::new();

    for (key, value) in pairs {
        let idx: u64 = key
            .parse()
            .map_err(|_| QpuError::Provider(std::format!("histogram key {key:?} is not an integer")))?;
        let p = value
            .as_f64()
            .ok_or_else(|| QpuError::Provider(std::format!("histogram value for {key:?} is not a number")))?;

        let mut label = String::with_capacity(m);
        for (pos, &q) in measured.iter().enumerate() {
            let _ = pos;
            // Bit `q` of the key is qubit `q`'s outcome, and wires are appended in ascending order —
            // which is exactly `measured`'s order.
            let bit = (idx >> q) & 1;
            label.push(if bit == 1 { '1' } else { '0' });
        }
        out.push((label, p));
    }

    Ok(out)
}

/// What stage a submitted job is at, mapped from IonQ's own `status` string.
fn stage_for(status: &str, queue: Option<u32>) -> Result<Option<Stage>, QpuError> {
    Ok(match status {
        "completed" => None,
        "submitted" | "ready" => Some(Stage::Queued(queue)),
        "running" => Some(Stage::Running),
        "canceled" | "cancelled" => return Err(QpuError::Cancelled),
        "failed" => return Err(QpuError::Provider("the provider reported the job failed".to_string())),
        other => {
            // An unknown status is treated as still-running rather than as success: assuming
            // completion would try to fetch results that do not exist.
            let _ = other;
            Some(Stage::Running)
        }
    })
}

/// Whether a target name denotes a classical simulator.
///
/// ⚠️ The single most consequential string comparison in this file. If it returns `false` for a
/// simulator, `QpuInfo::remote_is_simulator` is clear, the status renders as `REMOTE/hw`, and a
/// classical simulation is presented as a quantum measurement.
///
/// Conservative by construction: anything whose name is not clearly a real processor is treated as a
/// simulator. IonQ's hardware targets are all prefixed `qpu.`, so the test is "is it a `qpu.`" rather
/// than "does it contain `simulator`" — an unrecognised future target name then errs toward claiming
/// less, not more.
pub fn target_is_simulator(name: &str) -> bool {
    !name.starts_with("qpu.")
}

/// The IonQ backend.
pub struct IonqProvider {
    token: String,
    backend_name: String,
    target: ProviderTarget,
    next: u64,
    jobs: Vec<IonqJob>,
}

struct IonqJob {
    handle: JobHandle,
    id: Option<String>,
    body: Vec<u8>,
    measured: Vec<u8>,
    shots: u32,
    done: bool,
    /// Consecutive transient failures while polling. Reset on any successful call.
    transient: u32,
}

/// How many consecutive network/auth failures to absorb while watching a submitted job.
///
/// ★ The first version treated **any** polling error as fatal, which on a lossy link turns one
/// dropped packet into a dead job — except the job is not dead. The provider has it, is running it,
/// and on metered hardware is billing for it; we would just have stopped watching.
/// Raised 6 → 40 (2026-09-21) for the same reason as IBM's: six retries is about twenty seconds,
/// which is nothing on a lossy link, and abandoning a submitted job discards a paid-for result. The
/// caller's overall time budget is the real bound.
const MAX_TRANSIENT: u32 = 40;

/// Whether an error is worth retrying rather than abandoning the job for.
fn is_transient(e: &QpuError) -> bool {
    matches!(e, QpuError::Provider(_) | QpuError::Auth(_) | QpuError::Unreachable(_))
}

/// Translate a transport failure, preserving whether anything reached the wire.
///
/// ★ [`QpuError::Unreachable`] means the request provably never left this machine, which is what
/// makes re-sending a **POST** safe. Anything else might have been received and answered, and
/// replaying it would create a second job — on metered hardware, a second charge.
fn transport_err(e: nyx_net::Error) -> QpuError {
    if e.is_before_send() {
        QpuError::Unreachable(std::format!("{e}"))
    } else {
        QpuError::Provider(std::format!("{e}"))
    }
}

impl IonqProvider {
    /// Open a session against one target.
    ///
    /// `now_unix` is checked before anything is sent: a wrong clock makes every certificate fail as
    /// "not yet valid", and that surfaces as a generic handshake error which reads like a network
    /// fault. See `provider::clock_looks_sane`.
    pub fn open(target: ProviderTarget, now_unix: u64) -> Result<IonqProvider, QpuError> {
        clock_looks_sane(now_unix)?;
        require_https(API_ROOT)?;
        let token = Credentials::load()
            .get("ionq.token")
            .map(|s| s.to_string())
            .ok_or_else(|| {
                QpuError::Auth(
                    "no IonQ token: add `ionq.token = …` to quantum-credentials.txt and rebuild, \
                     or run `quantum remote login ionq <token>`"
                        .to_string(),
                )
            })?;
        Ok(IonqProvider {
            token,
            backend_name: target.name.clone(),
            target,
            next: 1,
            jobs: Vec::new(),
        })
    }

    /// The device description for this target.
    pub fn device_info(&self, id: u32) -> QpuInfo {
        let mut i = QpuInfo {
            id,
            kind: QuantumDeviceKind::Qpu as u32,
            status: QpuStatus::Remote as u32,
            // ★ The field that keeps a cloud simulator from reading as a quantum processor.
            remote_is_simulator: u32::from(self.target.is_simulator),
            qubits: self.target.qubits,
            // Trapped-ion devices are genuinely all-to-all; a provider simulator is too.
            topology: Topology::AllToAll as u32,
            exec_model: ExecModel::Batch as u32,
            gate_set: supported_gate_mask(),
            max_shots: 0,
            max_depth: 0,
            queue_depth: self.target.queue.unwrap_or(u32::MAX),
            queue_capacity: 0,
            // Not reported by this path. `0` means "not reported", never a plausible default — a
            // fabricated coherence time on a REMOTE/hw device would be the worst kind of wrong.
            coherence_t1_ns: 0,
            coherence_t2_ns: 0,
            calibrated_unix: 0,
            ..QpuInfo::default()
        };
        i.set_vendor("IonQ");
        i.set_arch(if self.target.is_simulator { "cloud-simulator" } else { "trapped-ion" });
        i.set_name(&self.target.name);
        i
    }

    fn auth_header(&self) -> String {
        std::format!("apiKey {}", self.token)
    }

    /// Decide whether a polling error ends the job or is worth another go.
    ///
    /// ★ A failure to *reach* the provider says nothing about the job. When we do give up, the
    /// message says the job may still be running — because it may, and a user who believes it died
    /// will not go and look for it.
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
            "lost contact with IonQ after {MAX_TRANSIENT} attempts ({e}). \
             job {id} may still be running on their hardware — check your IonQ dashboard"
        )))
    }

    /// One HTTPS request against the API.
    ///
    /// ⚠️ **Blocks** for up to `nyx_net`'s whole-request deadline. See [`IonqProvider::poll`].
    fn call(&self, method_post: bool, path: &str, body: Vec<u8>) -> Result<Value, QpuError> {
        let url_text = std::format!("{API_ROOT}{path}");
        require_https(&url_text)?;
        let url = nyx_net::Url::parse(&url_text)
            .map_err(|e| QpuError::Provider(std::format!("bad URL: {e}")))?;

        let req = if method_post {
            nyx_net::Request::post(body, "application/json")
        } else {
            nyx_net::Request::get()
        }
        .header("Authorization", &self.auth_header());

        let resp = nyx_net::request_once(&url, &req).map_err(transport_err)?;

        // Map HTTP status before parsing: a 401's body is not a job.
        match resp.status {
            200..=299 => {}
            401 | 403 => {
                return Err(QpuError::Auth(
                    "the provider rejected the token (401/403); check `quantum remote status`"
                        .to_string(),
                ))
            }
            429 => {
                return Err(QpuError::Provider(
                    "the provider is rate-limiting this machine (429)".to_string(),
                ))
            }
            s => {
                // ⚠️ The body may contain the token in an echoed request. Only the status is
                // reported, never the response text.
                return Err(QpuError::Provider(std::format!("the provider returned HTTP {s}")));
            }
        }

        let text = std::string::String::from_utf8(resp.body)
            .map_err(|_| QpuError::Provider("the response was not UTF-8".to_string()))?;
        parse(&text).map_err(|e| QpuError::Provider(std::format!("malformed JSON from provider: {e}")))
    }
}

impl QuantumProvider for IonqProvider {
    fn name(&self) -> &str {
        "ionq"
    }

    fn targets(&mut self) -> Result<Vec<ProviderTarget>, QpuError> {
        Ok(vec![self.target.clone()])
    }
}

impl QpuBackend for IonqProvider {
    fn name(&self) -> &str {
        "ionq"
    }

    fn describe(&self) -> &str {
        if self.target.is_simulator {
            "IonQ cloud, simulator target - REMOTE but still a classical simulation"
        } else {
            "IonQ cloud, trapped-ion processor - a real quantum device, reached over HTTPS"
        }
    }

    fn info(&self) -> QpuInfo {
        self.device_info(0)
    }

    fn begin(&mut self, c: &Circuit, shots: Shots, _seed: u64) -> Result<JobHandle, QpuError> {
        // Validate and serialise BEFORE anything is sent: a circuit the provider would reject is
        // better refused locally than submitted, queued, and billed.
        self.validate(c, shots)?;
        let body = submission_body(c, shots, &self.backend_name)?.to_string().into_bytes();

        let h = JobHandle(self.next);
        self.next += 1;
        self.jobs.push(IonqJob {
            handle: h,
            id: None,
            body,
            measured: c.measured_qubits(),
            shots: shots.0,
            done: false,
            transient: 0,
        });
        Ok(h)
    }

    /// Advance the job by one HTTPS round trip.
    ///
    /// ⚠️ **This blocks for the duration of one request** — typically well under a second, bounded by
    /// `nyx_net`'s whole-request deadline. That is coarser than `libs/net`'s `Fetch`, which slices
    /// itself into 20 ms socket timeouts.
    ///
    /// So: this is safe to drive from a terminal command that has announced it will block (the
    /// `wifi scan` precedent), and it must **not** be pumped from `apps/shell`, which *is* the window
    /// server. Making it `Fetch`-grained would mean reimplementing the HTTP state machine for POST;
    /// that is the right next step and is recorded in `docs/quantum/remote.md`.
    fn poll(&mut self, h: JobHandle) -> JobState {
        let Some(i) = self.jobs.iter().position(|j| j.handle == h) else {
            return JobState::Failed(QpuError::NoSuchJob(h));
        };
        if self.jobs[i].done {
            return JobState::Failed(QpuError::NoSuchJob(h));
        }

        // Stage 1: submit.
        if self.jobs[i].id.is_none() {
            let body = self.jobs[i].body.clone();
            return match self.call(true, "/jobs", body) {
                // ★ Retry ONLY when the request provably never left this machine — see
                // `transport_err`. ⚠️ Anything else might mean IonQ received the job and we lost the
                // reply; replaying a POST then would queue the same circuit twice and bill twice.
                Err(e @ QpuError::Unreachable(_)) => self.absorb(i, e, Stage::Submitting),
                Err(e) => {
                    self.jobs[i].done = true;
                    JobState::Failed(e)
                }
                Ok(v) => match v.get("id").and_then(|x| x.as_str()) {
                    None => {
                        self.jobs[i].done = true;
                        JobState::Failed(QpuError::Provider(
                            "the provider accepted the job but returned no id".to_string(),
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

        // Stage 2: poll status.
        //
        // ⚠️ A transport failure here does NOT end the job. The job is running on IonQ's hardware
        // whatever this link is doing, and abandoning it would discard a result we may already have
        // been billed for. See `MAX_TRANSIENT`.
        let status_doc = match self.call(false, &std::format!("/jobs/{id}"), Vec::new()) {
            Ok(v) => {
                self.jobs[i].transient = 0;
                v
            }
            Err(e) => return self.absorb(i, e, Stage::Queued(None)),
        };
        let status = status_doc.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
        let queue = status_doc.get("queue_position").and_then(|v| v.as_u64()).map(|v| v as u32);

        match stage_for(status, queue) {
            // The provider's own verdict on the job IS terminal, unlike a network opinion about it.
            Err(e) => {
                self.jobs[i].done = true;
                return JobState::Failed(e);
            }
            Ok(Some(stage)) => return JobState::Running(stage),
            Ok(None) => {}
        }

        // Stage 3: fetch results. Same treatment — the answer exists on IonQ's side now.
        let results = match self.call(false, &std::format!("/jobs/{id}/results/probabilities"), Vec::new()) {
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

        match decode_histogram(&results, &measured) {
            Err(e) => JobState::Failed(e),
            Ok(probs) => JobState::Done(std::boxed::Box::new(Outcome {
                // ★ Probabilities, NOT counts. The provider does not say how many shots produced
                // each bucket, and multiplying by `shots` would invent a measurement.
                readout: Readout::Probabilities(probs),
                shots_requested: shots,
                measured,
                provenance: Provenance::from_info("ionq", &info),
            })),
        }
    }

    fn cancel(&mut self, h: JobHandle) {
        if let Some(i) = self.jobs.iter().position(|j| j.handle == h) {
            // Best-effort: tell the provider, then forget it locally regardless. A cancel that fails
            // to reach the API must still stop this session from polling.
            if let Some(id) = self.jobs[i].id.clone() {
                let _ = self.call(true, &std::format!("/jobs/{id}/status/cancel"), Vec::new());
            }
            self.jobs.remove(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bell_circuit_serialises_to_the_documented_shape() {
        let body = submission_body(&Circuit::bell(), Shots(1024), "simulator").unwrap();
        let text = body.to_string();

        // Verified against docs.ionq.com (v0.4) on 2026-09-12.
        assert!(text.contains(r#""type":"ionq.circuit.v1""#), "{text}");
        assert!(text.contains(r#""backend":"simulator""#), "{text}");
        assert!(text.contains(r#""gateset":"qis""#), "{text}");
        assert!(text.contains(r#""shots":1024"#), "shots must be an integer: {text}");
        assert!(text.contains(r#""qubits":2"#), "{text}");
        assert!(text.contains(r#"{"gate":"h","target":0}"#), "{text}");
        assert!(text.contains(r#"{"gate":"cnot","control":0,"target":1}"#), "{text}");
    }

    /// ⚠️ Measurements are not instructions on this API — IonQ measures everything at the end.
    #[test]
    fn measurements_are_not_sent_but_still_decide_the_labels() {
        let c = Circuit::bell();
        let arr = serialise_circuit(&c).unwrap();
        // H and CX only; the two Measure ops contribute nothing.
        assert_eq!(arr.as_array().unwrap().len(), 2);
        assert_eq!(c.measured_qubits(), vec![0, 1]);
    }

    #[test]
    fn an_unsupported_gate_is_refused_by_name_rather_than_guessed_at() {
        let mut c = Circuit::new(3, 3);
        c.gate(Gate::Ccx, &[0, 1, 2]).measure_all();
        match serialise_circuit(&c) {
            Err(QpuError::UnsupportedGate(g)) => assert_eq!(g, "ccx"),
            other => panic!("expected a named refusal, got {other:?}"),
        }

        let mut c = Circuit::new(1, 1);
        c.gate(Gate::U3(1.0, 2.0, 3.0), &[0]).measure_all();
        assert!(matches!(serialise_circuit(&c), Err(QpuError::UnsupportedGate("u3"))));
    }

    #[test]
    fn reset_is_refused_with_an_actionable_message() {
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::H, &[0]).reset(0).measure(0, 0);
        match serialise_circuit(&c) {
            Err(QpuError::Unsupported(why)) => {
                assert!(why.contains("reset"), "{why}");
                assert!(why.contains("local simulator"), "the message should offer a way out: {why}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rotations_carry_their_angle() {
        let mut c = Circuit::new(1, 1);
        c.gate(Gate::Rx(1.5), &[0]).measure(0, 0);
        let text = serialise_circuit(&c).unwrap().to_string();
        assert!(text.contains(r#""gate":"rx""#), "{text}");
        assert!(text.contains(r#""rotation":1.5"#), "{text}");
    }

    #[test]
    fn swap_uses_a_targets_array() {
        let mut c = Circuit::new(2, 2);
        c.gate(Gate::Swap, &[0, 1]).measure_all();
        let text = serialise_circuit(&c).unwrap().to_string();
        assert!(text.contains(r#""gate":"swap","targets":[0,1]"#), "{text}");
    }

    #[test]
    fn the_advertised_gate_set_is_exactly_what_can_be_serialised() {
        let mask = supported_gate_mask();
        for g in Gate::ALL {
            let claimed = nyx_quantum::circuit::gate_set_contains(mask, g);
            assert_eq!(
                claimed,
                gate_name(g).is_some(),
                "{} is advertised as {claimed} but serialisable as {}",
                g.qasm_name(),
                gate_name(g).is_some()
            );
        }
    }

    // ── the histogram ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_bell_histogram_decodes() {
        let v = parse(r#"{"0":0.4970703125,"3":0.5029296875}"#).unwrap();
        let got = decode_histogram(&v, &[0, 1]).unwrap();
        assert_eq!(got.len(), 2);
        let p = |l: &str| got.iter().find(|(k, _)| k == l).map(|(_, v)| *v).unwrap_or(0.0);
        assert!((p("00") - 0.4970703125).abs() < 1e-12);
        assert!((p("11") - 0.5029296875).abs() < 1e-12);
    }

    /// ★★ The test that can actually catch a bit-order mistake.
    ///
    /// Bell and GHZ outcomes are `00`/`11` and `000`/`111` — palindromes. Reversing the bit order
    /// leaves them completely unchanged, so they cannot detect the error. This one can.
    #[test]
    fn an_asymmetric_outcome_proves_the_bit_order() {
        // Key 1 on two qubits: bit 0 set means QUBIT 0 is 1. Nyx labels wire 0 first, so "10".
        let v = parse(r#"{"1":1.0}"#).unwrap();
        let got = decode_histogram(&v, &[0, 1]).unwrap();
        assert_eq!(got[0].0, "10", "little-endian key 1 must be wire0=1, wire1=0");

        // Key 2: bit 1 set means qubit 1 is 1 -> "01".
        let v = parse(r#"{"2":1.0}"#).unwrap();
        assert_eq!(decode_histogram(&v, &[0, 1]).unwrap()[0].0, "01");

        // Three qubits, key 4 = bit 2 = qubit 2 -> "001".
        let v = parse(r#"{"4":1.0}"#).unwrap();
        assert_eq!(decode_histogram(&v, &[0, 1, 2]).unwrap()[0].0, "001");
    }

    #[test]
    fn a_partial_measurement_labels_only_the_measured_wires() {
        // Measured wires 0 and 2 of a 3-qubit device. Key 5 = bits 0 and 2 set.
        let v = parse(r#"{"5":1.0}"#).unwrap();
        let got = decode_histogram(&v, &[0, 2]).unwrap();
        assert_eq!(got[0].0, "11");

        // Key 1 = only qubit 0 -> wire 0 is 1, wire 2 is 0.
        let v = parse(r#"{"1":1.0}"#).unwrap();
        assert_eq!(decode_histogram(&v, &[0, 2]).unwrap()[0].0, "10");
    }

    #[test]
    fn a_malformed_histogram_is_reported_not_silently_dropped() {
        assert!(decode_histogram(&parse("[]").unwrap(), &[0]).is_err());
        assert!(decode_histogram(&parse(r#"{"notanint":0.5}"#).unwrap(), &[0]).is_err());
        assert!(decode_histogram(&parse(r#"{"0":"half"}"#).unwrap(), &[0]).is_err());
    }

    // ── status mapping ──────────────────────────────────────────────────────────────────────────

    #[test]
    fn status_strings_map_to_stages() {
        assert!(matches!(stage_for("submitted", Some(3)), Ok(Some(Stage::Queued(Some(3))))));
        assert!(matches!(stage_for("ready", None), Ok(Some(Stage::Queued(None)))));
        assert!(matches!(stage_for("running", None), Ok(Some(Stage::Running))));
        assert!(matches!(stage_for("completed", None), Ok(None)));
        assert!(matches!(stage_for("failed", None), Err(QpuError::Provider(_))));
        assert!(matches!(stage_for("canceled", None), Err(QpuError::Cancelled)));
    }

    /// An unknown status must not be read as success — that would fetch results that do not exist.
    #[test]
    fn an_unknown_status_keeps_waiting_rather_than_claiming_completion() {
        assert!(matches!(stage_for("quantum_shenanigans", None), Ok(Some(Stage::Running))));
    }

    // ── the simulator/hardware distinction ──────────────────────────────────────────────────────

    /// ★ The most consequential string comparison in the file.
    #[test]
    fn only_qpu_prefixed_targets_count_as_hardware() {
        assert!(!target_is_simulator("qpu.aria-1"));
        assert!(!target_is_simulator("qpu.harmony"));
        assert!(!target_is_simulator("qpu.forte-1"));

        assert!(target_is_simulator("simulator"));
        assert!(target_is_simulator("simulator.noisy"));
        // ⚠️ Conservative on anything unrecognised: claim less, not more.
        assert!(target_is_simulator("something-new-in-2027"));
        assert!(target_is_simulator(""));
    }

    #[test]
    fn a_remote_simulator_is_labelled_as_one_all_the_way_to_the_disclosure() {
        // Built without `open()` so the test needs no token and no network.
        let p = IonqProvider {
            token: "x".to_string(),
            backend_name: "simulator".to_string(),
            target: ProviderTarget {
                name: "simulator".to_string(),
                qubits: 29,
                is_simulator: true,
                queue: None,
            },
            next: 1,
            jobs: Vec::new(),
        };
        let i = p.info();
        assert_eq!(i.status(), QpuStatus::Remote);
        assert!(i.remote_is_simulator());
        assert_eq!(i.status_label(), "REMOTE/sim");
        assert!(!i.is_quantum(), "a cloud simulator is not a quantum result");

        let prov = Provenance::from_info("ionq", &i);
        assert!(prov.disclosure().contains("SIMULATOR"), "{}", prov.disclosure());
        assert!(!prov.is_quantum);
    }

    #[test]
    fn a_real_processor_is_labelled_as_hardware_and_reports_no_invented_metrics() {
        let p = IonqProvider {
            token: "x".to_string(),
            backend_name: "qpu.aria-1".to_string(),
            target: ProviderTarget {
                name: "qpu.aria-1".to_string(),
                qubits: 25,
                is_simulator: false,
                queue: Some(14),
            },
            next: 1,
            jobs: Vec::new(),
        };
        let i = p.info();
        assert_eq!(i.status_label(), "REMOTE/hw");
        assert!(i.is_quantum());
        assert_eq!(i.arch_str(), "trapped-ion");
        assert_eq!(i.queue(), Some(14));
        // ★ Not reported is not zero. A fabricated coherence time on a device claiming to be real
        // hardware would be the worst possible place to invent a number.
        assert_eq!(i.coherence_t1(), None);
        assert_eq!(i.coherence_t2(), None);
        assert_eq!(i.calibrated(), None);

        assert!(Provenance::from_info("ionq", &i).disclosure().contains("remote quantum hardware"));
    }
}
