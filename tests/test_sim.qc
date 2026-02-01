// tests/test_sim.qc
fn main() -> int {
    // 1. Allocate two qubits (initially |0>)
    qubit q1 = |0>;
    qubit q2 = |0>;

    // 2. Put q1 into Superposition
    // State: (|00> + |10>) / sqrt(2)
    H(q1);

    // 3. Entangle q1 and q2
    // If q1 is 1, flip q2.
    // State: (|00> + |11>) / sqrt(2)
    CNOT(q1, q2);

    // 4. Measure
    // Due to entanglement, r1 and r2 MUST be equal.
    // You will get either (0, 0) or (1, 1).
    cbit r1 = measure(q1);
    cbit r2 = measure(q2);

    return 0;
}