// tests/teleport.qc
// Implements the Quantum Teleportation Protocol.

fn teleport(msg: qubit, alice: qubit, bob: qubit) -> () {
    // 1. Bell Measurement
    CNOT(msg, alice);
    H(msg);
    
    let m1: cbit = measure(msg);
    let m2: cbit = measure(alice);
    
    // 2. Classical Feed-Forward
    // Bob applies gates based on Alice's measurement results
    if (m2 == 1) { 
        X(bob); 
    }
    if (m1 == 1) { 
        Z(bob); 
    }

    // FIX: Explicit return required by QIR Builder
    return;
}

fn main() -> int {
    // Setup 3 qubits
    qubit msg = |1>;   // The state to teleport
    qubit alice = |0>; // Alice's half of the pair
    qubit bob = |0>;   // Bob's half of the pair

    // Create entanglement between Alice and Bob first
    H(alice);
    CNOT(alice, bob);

    // Run protocol
    teleport(msg, alice, bob);

    // Verify: Bob should now be |1>
    let result: cbit = measure(bob);
    
    if (result == 1) {
        return 1; // Success
    }
    return 0; // Failure
}