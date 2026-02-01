fn main() -> int {
    // --- TEST 1: Gate Cancellation ---
    // We measure q2 to keep this chain "live".
    // If H+H are not removed, we see 2 gates.
    // If Gate Cancellation works, we see 0 gates (just measurement).
    qubit q0 = |0>;
    let q1: qubit = H(q0);
    let q2: qubit = H(q1);
    cbit r_cancel = measure(q2); // Keep it live!

    // --- TEST 2: Dead Qubit Elimination ---
    // Not measured -> X gate should be removed.
    qubit q_dead = |0>;
    let q_dead_1: qubit = X(q_dead);
    
    // --- TEST 3: Control ---
    qubit q_live = |0>;
    let q_live_1: qubit = H(q_live);
    cbit r_live = measure(q_live_1);
    
    return 0;
}