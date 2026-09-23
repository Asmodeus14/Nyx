# QCLang Command Line Interface (CLI)

The `qclang` CLI is the primary tool for interacting with the QCLang compiler. It is designed for professional quantum systems programming, handling compilation, optimization, package management, and benchmarking.

## Global Options

These options apply to all subcommands.

| Option | Short | Description |
| --- | --- | --- |
| `--verbose` | `-v` | Enable verbose logging (shows detailed compilation stats, paths, and git info). |
| `--no-color` |  | Disable colored terminal output (useful for CI/CD logs or file redirection). |
| `--silent` |  | Suppress all output except errors (hides banners and progress bars). |

---

## Commands

### 1. `compile`

Compiles QCLang source files (`.qc`) into OpenQASM 2.0. This is the core function of the tool.

**Usage:**

```bash
qclang compile [OPTIONS] <INPUT_FILES>...

```

**Arguments:**

* `<INPUT_FILES>`: One or more `.qc` source files to compile.

**Options:**

* `-o, --output <PATH>`: Specify the output directory. If omitted, files are saved alongside the source.
* `-O, --optimize`: **Enable Phase 2 Optimizations** (Dead Qubit Elimination, Gate Cancellation).
* `-s, --show`: Print the generated code to stdout immediately after compilation.
* `-f, --format <FORMAT>`: Output format. Default is `qasm`.
* `qasm`: Standard OpenQASM 2.0.
* `json`: Metadata JSON (qubit counts, gate depth).
* `qir`: (Experimental) Quantum Intermediate Representation.



**Example:**

```bash
# Compile with high-level optimizations and view output
qclang compile main.qc -O --show

```

### 2. `run`

Compiles a single file, runs compiler passes, and displays a detailed circuit topography report.

**Usage:**

```bash
qclang run [OPTIONS] <INPUT_FILE>

```

**Options:**

* `--simulate`: Trigger the simulation backend (Simulates measurement outcomes).
* `--no-opt`: Explicitly disable optimizations for this run (useful for debugging raw circuit logic).

**Example:**

```bash
qclang run circuit.qc --simulate

```

### 3. `update`

Manages the QCLang installation. It connects to the official repository to fetch and install the latest binaries.

**Usage:**

```bash
qclang update [OPTIONS]

```

**Behaviors:**

* **Interactive Mode:** If run without arguments, it fetches the release history and presents an interactive menu to select a version.
* **Auto-Update:** Use flags to bypass the menu.

**Options:**

* `--latest`: Automatically select and install the newest available version.
* `--tag <TAG>`: Install a specific release tag (e.g., `v0.5.0`).
* `--force`: Force re-installation even if the version matches the current one.

**Example:**

```bash
# Update to the absolute latest version automatically
qclang update --latest

```

### 4. `check`

Performs lexical analysis, parsing, and semantic verification without generating output code. Use this for quick syntax validation during development (linter mode).

**Usage:**

```bash
qclang check [OPTIONS] <INPUT_FILES>...

```

**Options:**

* `--ast`: Print the Abstract Syntax Tree (AST) structure upon success.

### 5. `info`

Displays metadata about the current QCLang installation, including:

* Repository URL
* License information
* Build architecture (e.g., `x86_64`)
* Active optimization passes

**Usage:**

```bash
qclang info

```

### 6. `benchmark`

Runs performance benchmarks on the compiler itself to measure throughput (compilations per second) and latency.

**Usage:**

```bash
qclang benchmark [OPTIONS]

```

**Options:**

* `-i, --iterations <NUM>`: Number of compile cycles to run (Default: 1000).

**Output:**
Displays a structured table with Average Compile Time (ms), Throughput (ops/sec), and Total Time.

### 7. `test`

Runs the internal compiler verification suite.

**Usage:**

```bash
qclang test [OPTIONS]

```

**Options:**

* `-p, --pattern <STRING>`: Run only tests matching the given pattern.
* `--report`: Generate a JSON/Markdown report of the test results.

### 8. `repl`

Starts the **Read-Eval-Print Loop**. Allows entering QCLang functions interactively for immediate compilation and analysis.

**Usage:**

```bash
qclang repl

```

**Commands inside REPL:**

* Type any valid QCLang function (e.g., `fn main() -> int { ... }`).
* `quit` / `exit`: Close the session.

### 9. `capabilities`

Lists the supported quantum gates, hardware backends, and language features enabled in this build.

**Usage:**

```bash
qclang capabilities

```

---

## Exit Codes

* `0`: Success.
* `1`: Compilation/Runtime Error.
* `101`: Internal panic or missing system dependency (e.g., OpenSSL on Linux).
