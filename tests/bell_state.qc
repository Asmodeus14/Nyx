// tests/bell_state.qc
// Creates a maximally entangled Bell pair (|00> + |11>) / sqrt(2).

fn main() -> int {
    // 1. Initialize two qubits to |0>
    qubit q1 = |0>;
    qubit q2 = |0>;

    // 2. Create Superposition on q1
    H(q1);

    // 3. Entangle q1 and q2
    // If q1 was 0, q2 stays 0. If q1 was 1, q2 flips to 1.
    CNOT(q1, q2);

    // 4. Measure results
    let r1: cbit = measure(q1);
    let r2: cbit = measure(q2);

    // 5. Verify correlation (Classically)
    if (r1 == r2) {
        // They matched! Entanglement confirmed.
        return 1;
    }

    return 0;
}