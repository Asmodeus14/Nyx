# Remote quantum computing

`libs/quantum-rt/src/provider.rs` and `libs/quantum-rt/src/ionq.rs`.

## Why this is a primary deliverable and not a future phase

Because it is the **only** way Nyx can produce a result that was actually computed by quantum
mechanics. There is no consumer gate-model QPU this OS can drive over PCIe — `limitations.md` has the
full account — so `QpuStatus::Hardware` is unreachable here by design.

A cloud processor is reachable. Nyx has working TLS 1.3 on real hardware (`libs/net`, rustls +
rustls-rustcrypto + webpki-roots, verified on this laptop 2026-09-10).

```
Nyx application → QpuSession → IonqProvider → libs/json → libs/net (TLS 1.3) → Internet → real QPU
```

The kernel is not in that chain. No token, URL or circuit reaches ring 0.

## ⚠️ A remote simulator is not a quantum result

The trap the whole device model is shaped around.

Providers serve classical simulators **through the same API, with the same job lifecycle, returning the
same JSON** as their real processors. IonQ's `simulator` and its `qpu.aria-1` differ by one string in
one field.

So `ProviderTarget::is_simulator` becomes `QpuInfo::remote_is_simulator`, which renders as
`REMOTE/sim` rather than `REMOTE/hw`, and makes `is_quantum()` false. Without it, a classical
simulation would read as a quantum measurement.

`ionq::target_is_simulator` tests `!name.starts_with("qpu.")` rather than looking for the word
"simulator" — so an unrecognised future target name errs toward **claiming less**, not more.

## Why IonQ first

1. **Its wire format is nearly Nyx's own IR.** A circuit is a JSON array of
   `{"gate":"h","target":0}`, so serialisation is a match arm per gate rather than a compiler. Compare
   IBM's Qiskit Runtime, which wants circuits already transpiled to a device's basis gates.
2. **Authentication is one header.** `Authorization: apiKey <token>`. AWS Braket needs SigV4 request
   signing, which needs HMAC-SHA256 — and `libs/crypto` has only SHA-1, HMAC-SHA1 and PBKDF2-SHA1. So
   Braket is not reachable from this tree without new cryptography. Azure needs OAuth2 plus a blob
   upload.

## API version, and what was actually verified

Written against **v0.4**, read from `docs.ionq.com` on 2026-09-12. The shape changed from v0.3:

| | v0.3 | v0.4 |
|---|---|---|
| submission key | `format: "ionq.circuit.v0"` | `type: "ionq.circuit.v1"` |
| target field | `target` | `backend` |
| gateset | implicit | `gateset: "qis"` |
| results | `/jobs/{id}/results` | `/jobs/{id}/results/probabilities` |

```
POST https://api.ionq.co/v0.4/jobs
  Authorization: apiKey <token>      Content-Type: application/json
  {"type":"ionq.circuit.v1","name":"nyx","shots":1024,"backend":"simulator",
   "input":{"qubits":2,"gateset":"qis",
            "circuit":[{"gate":"h","target":0},{"gate":"cnot","control":0,"target":1}]}}
→ {"id":"<uuid>","status":"submitted"}

GET  /v0.4/jobs/<id>                       poll until status == "completed"
GET  /v0.4/jobs/<id>/results/probabilities  → {"0":0.497,"3":0.503}
POST /v0.4/jobs/<id>/status/cancel
```

### ⚠️ What was verified and what was not

**Verified against the live documentation:** the endpoint paths, the submission body shape, the
`Authorization` form, the `h` and `cnot` gate encodings, and that results are probabilities keyed by
little-endian integers.

**Not verified:** the remaining abstract-gateset gate *names*. They follow IonQ's published `qis` gate
list and are encoded in exactly one table — `ionq::gate_name` — and **any gate not in that table is
refused by name** rather than guessed at, so an unverified spelling cannot silently produce a circuit
that means something else.

`Ccx` and `U3` are refused for this reason: Toffoli needs a multi-control field name that was not
verified, and a three-qubit gate silently encoded as a two-qubit one would be a wrong answer rather
than an error. `U3` has no single abstract equivalent and would need decomposing, which is a
transpiler.

**The first real submission should be checked against the job's own `gate_counts` field.** That is the
cheapest way to confirm the gate names round-tripped.

`the_advertised_gate_set_is_exactly_what_can_be_serialised` asserts that `QpuInfo::gate_set` and
`gate_name` cannot drift apart, so a device never claims a gate the serialiser would refuse.

## ★★ The histogram: probabilities, little-endian

Two things to get right, both easy to get wrong in a way that still looks plausible.

**Probabilities, not counts.** The response does not say how many shots produced each bucket.
Multiplying by the requested shot count to print "509 of 1024" would invent a measurement — so this
returns `Readout::Probabilities` and the terminal prints probabilities with the shot count stated
separately as what was *asked for*.

**Little-endian keys.** The key is an integer whose **bit `i` is qubit `i`**. Nyx's labels read left to
right as wire 0 first, so on two qubits key `1` is `"10"` — not `"01"`.

⚠️ **Bell and GHZ results cannot catch a mistake here**, because `00`/`11` and `000`/`111` are
palindromes: reversing the bit order leaves them unchanged. `an_asymmetric_outcome_proves_the_bit_order`
is the test that can, using keys `1`, `2` and `4`.

## Job lifecycle

`Submitting → Queued(position) → Running → Fetching → Done`, each stage announced before it executes.

`stage_for` maps IonQ's `status` string. ⚠️ An **unknown** status is treated as still-running rather
than as success: assuming completion would fetch results that do not exist.

`cancel` is best-effort — it tells the provider, then forgets the job locally regardless, so a cancel
that fails to reach the API still stops this session polling.

## ⚠️ Blocking, and the honest statement of it

Each `poll()` performs **one HTTPS round trip** and blocks for its duration, bounded by `libs/net`'s
whole-request deadline. That is far coarser than `Fetch`, which slices itself into 20 ms socket
timeouts.

So:

- It is safe from a terminal command that has **announced** it will block — the `wifi scan` precedent,
  which prints "Scanning every channel (about seven seconds)..." before it starts.
- It must **not** be pumped from `apps/shell`, which *is* the window server.

Making it `Fetch`-grained means reimplementing the stepped HTTP state machine for POST. That is the
right next step and is not done.

The terminal's `quantum remote run` loop is bounded at 120 polls with a 1 s sleep, and cancels the job
if it gives up, so a stuck provider cannot wedge the terminal forever.

## Security

- **HTTPS only.** `require_https` refuses a non-HTTPS URL rather than downgrading. A token travels in a
  header on every request; over plaintext that is the credential handed to anyone on the path.
- **Clock check first.** ⚠️ A wrong RTC makes **every** certificate fail as "not yet valid", which
  surfaces from rustls as a generic handshake error that reads like a network problem. This has cost
  the project a debugging session before. `clock_looks_sane` checks for a timestamp before 2020 and
  says *"run `time sync`"*.
- **The token is never printed in full.** `TokenStore::redact` shows the last four characters.
  Provider error bodies are **not** echoed either — only the HTTP status — because a 4xx body can
  contain an echoed request carrying the token.
- **Tokens with CR/LF are refused at storage**, where the message can explain, rather than being
  silently dropped later by `Request::header`'s injection guard and producing a 401 that looks like a
  wrong key.
- **`request_once` never retries.** `Fetch` replays a request once when a reused keep-alive connection
  turns out to be closed — safe for a GET, and **not** safe for a POST. A replayed job submission is a
  second job, and on metered hardware a second charge. `Request::is_idempotent` encodes the rule.

## IBM Quantum

Worth the extra work for one reason: **the Open plan includes 10 free minutes per 28 days on real
QPUs.** IonQ's simulator is free but is a simulator; its `qpu.*` targets are metered. A Bell pair at
1024 shots is a few seconds, so ten minutes a month is a lot of genuine measurements.

It is harder than IonQ in two places.

**Authentication is a two-step exchange.** `POST https://iam.cloud.ibm.com/identity/token`,
**form-encoded** — the only non-JSON request in this subsystem — swapping the API key for a bearer
token that expires in 3600 seconds. Every subsequent request carries `Authorization: Bearer …` **and**
`Service-CRN: …`; the instance CRN is a *second* credential, ~120 characters. Nyx names the two
separately in its errors, because a valid key with no CRN otherwise fails in a way that reads exactly
like a rejected key.

### ★★ IBM requires ISA circuits, and that is a compiler

IonQ accepts abstract gates and transpiles server-side. IBM does not: the Runtime primitives want a
circuit already decomposed to the backend's basis gates *and* already mapped onto physical qubits,
submitted as OpenQASM 3 addressing hardware qubits (`$0`, `$1`) rather than virtual registers.

Heron's basis set is **`cz`, `id`, `rz`, `sx`, `x`** on a **heavy-hex** lattice where a qubit has at
most three neighbours. A general solution therefore needs decomposition *and* routing — inserting
SWAPs where a two-qubit gate spans non-adjacent physical qubits. Routing is a real algorithm, it is
bug-prone, and its quality directly affects fidelity, because every inserted gate adds error.

**`libs/quantum-rt/src/ibm.rs` deliberately does not route.** It handles the circuits that need none:

- **Decomposition** is fixed arithmetic: `h → rz(π/2)·sx·rz(π/2)`, `cx(a,b) → h(b)·cz(a,b)·h(b)`,
  `z → rz(π)`, `s → rz(π/2)`, `t → rz(π/4)`, `y → rz(π)·x`. Exact up to a global phase, which is
  unobservable in measurement statistics.
- **Placement** (`place_on_coupling_map`) finds a *connected path* in the backend's coupling map so
  logical `i` and `i+1` are always adjacent, then verifies every two-qubit gate joins a logically
  adjacent pair.

That covers **Bell pairs and GHZ chains** and refuses everything else **by name**. `swap` and `ccx`
are excluded even though both are expressible, because each would produce two-qubit gates that no
longer sit on a single edge.

⚠️ A circuit this cannot place is rejected, never silently mis-compiled. A wrong placement still
returns a perfectly plausible histogram — of a different computation.

```
POST https://quantum.cloud.ibm.com/api/v1/jobs
  Authorization: Bearer <iam-token>     Service-CRN: <crn>
  {"program_id":"sampler","backend":"ibm_fez",
   "params":{"pubs":[["<OPENQASM 3 ISA circuit>",null,1024]]}}
```

⚠️ **IBM returns per-shot samples, not probabilities** — the opposite of IonQ — so its readout really
is `Readout::Counts`. The samples are hex strings of the classical register, with the same
little-endian convention: bit `i` is classical bit `i`.

**Not verified** against a live account: the coupling-map endpoint and the exact results document
shape. Both fail loudly rather than guessing. `coupling_map()` refuses rather than assuming qubits 0
and 1 are adjacent — that is true on every IBM lattice I know of, and *"true on every one I know of"*
is not a basis for deciding where a computation runs.

## Credentials

Two files, and the distinction is load-bearing:

| file | in the initrd tar? | lifetime |
|---|---|---|
| `/mnt/nvme/etc/quantum-credentials.baked` | **yes** | rewritten from the image every boot |
| `/mnt/nvme/etc/quantum-credentials` | **no** | written on-device, survives reboots |

`installer::extract_tar_to_ext4` walks the tar and writes each entry — it does **not** wipe the
filesystem first. So a file not in the tar survives, and one that is gets refreshed. `CredSet::merge`
reads the baked file then overlays the runtime one, so an on-device edit wins and persists, and
reflashing cannot silently revert it.

### ★ Why credentials arrive with the image

**Nyx has no clipboard and cannot express a paste chord** — `HandleControl::Ignore` means `Ctrl+V` is
not an input event at all. An IonQ key is ~40 characters, an IBM key ~44, and an IBM CRN ~120. Typing
that by hand, with no paste and no way to correct a mistake you cannot see, is not a workflow anyone
uses twice.

So the primary path is build-time. Copy the tracked template to a **gitignored** file and rebuild:

```
cp quantum-credentials.example.txt quantum-credentials.txt   # then edit
./Build.sh
  [creds] baked into the image: ionq.token ibm.token ibm.crn
```

`Build.sh` reports which **key names** were found, never their values. The absent case is reported
too, because a build that silently ships no key produces a runtime error that looks like a network
fault.

⚠️ `quantum-credentials.txt` is in `.gitignore` and must never be moved into source. `git log -p`
keeps a deleted line forever, and a leaked key on a metered backend is someone else's bill.

Format: `key = value`, `#` comments. **Only the first `=` splits** — a CRN's own colons, slashes and
trailing `::` have to survive intact.

### On-device

`quantum remote set <ionq.token|ibm.token|ibm.crn> <value>` writes the runtime file; `quantum remote
clear` forgets it, reverting to the image. Settings › Quantum shows the same state with a
**Reload**/**Clear** pair and deliberately **no text field**: the terminal already has a working
input with history, and a hand-rolled field would not make a 120-character CRN any more typeable.

Parsing lives in `nyx_quantum::creds` — `no_std`, no filesystem — because `libs/quantum-rt` reads
these files through `std::fs` and `apps/settings` through `nyx_api::sys_open`. One implementation, so
the screen and the command line cannot disagree about what is configured.

⚠️ **Nyx has no permission model.** `struct Process` has no uid, and not one of ~130 syscall arms
consults caller identity, so these files are readable by any process on the machine. Pretending
otherwise would be worse than the exposure, so the warning is printed at the moment of storage *and*
shown permanently in Settings.

## Adding another provider

Implement `QuantumProvider` + `QpuBackend`. The five operations every provider has are: list targets,
submit, poll status, fetch results, cancel. What differs is authentication and the circuit wire format.

| provider | status |
|---|---|
| IonQ | **done** — abstract gates, one auth header |
| IBM Quantum | **done for Bell/GHZ** — ISA circuits, no SWAP router |
| AWS Braket | SigV4 signing → needs HMAC-SHA256 in `libs/crypto` |
| Azure Quantum | OAuth2 + blob upload |
| Rigetti / Quantinuum | not investigated |

![Nyx's terminal on the test laptop: quantum remote jobs, then a Bell circuit's result from IBM's ibm_fez — 00: 502, 11: 463, 01: 43, 10: 16, "executed on remote quantum hardware"](../images/hw-ibm-bell.jpg)

<sub>`quantum remote jobs` on the test laptop, collecting a Bell circuit run on IBM's `ibm_fez`. The
`01`/`10` counts (43 and 16 of 1024) are real-device noise; the local simulator reports exactly zero
there. Filmed with a phone — see the [video](../media/nyx-hardware-terminal.mp4).</sub>

## Not implemented

- **Target enumeration.** There is no `quantum remote devices`, because no backend-listing endpoint was
  verified and inventing one — or inventing qubit counts for targets — would be a fabrication.
  `quantum remote run <target>` names the target directly, and `ProviderTarget::qubits` is `0`
  ("not reported") rather than a guess.
- **A `Fetch`-grained POST state machine.** See the blocking note above.
- **Multi-circuit jobs**, batching, and error mitigation.
