// tests/registers.qc
// Demonstrates quantum registers and loops.

fn main() -> int {
    // 1. Initialize a 4-qubit register
    qreg q[4] = |0000>;

    // 2. Apply X (NOT) gate to even indices only
    // Creates state |1010>
    for i in range(0, 4) {
        // Classical modulo arithmetic check
        if ((i / 2) * 2 == i) { 
             X(q[i]);
        }
    }

    // 3. Measure the whole register
    let mut score: int = 0;
    
    for i in range(0, 4) {
        let m: cbit = measure(q[i]);
        if (m == 1) {
            score += 1;
        }
    }

    return score; // Should return 2
}