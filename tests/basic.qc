// tests/basics.qc
// Tests classical variable declarations, arithmetic, and control flow.

fn main() -> int {
    // 1. Variable Declarations
    let x: int = 10;
    let mut y: int = 20;
    
    // 2. Arithmetic & Assignment
    y += x;          // y is now 30
    let z: int = y * 2; // z is 60
    
    // 3. Control Flow (If/Else)
    if (z > 50) {
        y = y - 1;
    } else {
        y = y + 1;
    }
    
    // 4. Loops
    let mut sum: int = 0;
    for i in range(0, 5) {
        sum += i;
    }
    
    // 5. Return
    return sum; // Should return 10
}