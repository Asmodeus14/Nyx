fn main() {
    println!("🚀 Hello from Standard Rust on NyxOS!");
    
    let mut numbers = Vec::new();
    for i in 1..=3 {
        numbers.push(i * 10);
    }
    
    println!("Rust Heap Allocation successful! Data: {:?}", numbers);
}