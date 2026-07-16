// qcstudio sample: a Bell pair (entangle two qubits, then measure both).
// Compiled on-device by qcstudio (Workstream C); its .qasm output is byte-compared
// against host qclang for the same input in C3 validation.
//
// QCLang uses affine (linear) typing for quantum resources: a qubit cannot be
// reassigned, so gates are applied as fresh bindings (`qubit c = CNOT(H(a), b);`)
// rather than mutation (`a = H(a);`).
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = CNOT(H(a), b);
    cbit r1 = measure(a);
    cbit r2 = measure(b);
    return 0;
}
