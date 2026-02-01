// tests/structs.qc
// Tests struct definitions containing quantum resources.

struct QNode {
    id: int,
    q: qubit,
};

fn prepare_node(id: int) -> QNode {
    // Initialize a new qubit
    qubit temp = |0>;
    
    // Set state based on ID
    if (id > 5) {
        X(temp);
    }
    
    // Return struct constructed with struct literal syntax
    return QNode {
        id: id,
        q: temp,
    };
}

fn main() -> int {
    let node: QNode = prepare_node(10);
    
    // Access members
    let val: int = node.id;
    let res: cbit = measure(node.q);
    
    return 0;
}