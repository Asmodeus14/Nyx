
---

# QCLang Language Specification (v0.6.0)

## 1. Lexical Elements & Primitive Syntax

The lexer identifies the following tokens. Each category below shows the internal token name, the matching pattern, and how it appears in code.

### 1.1 Literals

| Token Type | Lexer Pattern (Regex/Token) | Description | Code Example |
| --- | --- | --- | --- |
| `IntLiteral` | `[0-9]+` | Standard 64-bit integers. | `42` |
| `FloatLiteral` | `[0-9]+\.[0-9]*` | Floating-point numbers. | `3.14159` |
| `StringLiteral` | `"[^"]*"` | Double-quoted strings. | `"Hello, QC!"` |
| `QubitLiteral` | `|[01]+>` | Quantum state initialization. | `|0>`, `|110>` |

### 1.2 Identifiers

| Token Type | Lexer Pattern | Description | Code Example |
| --- | --- | --- | --- |
| `Ident` | `[a-zA-Z_][a-zA-Z0-9_]*` | Variable, function, and type names. | `my_qubit`, `result1` |

---

## 2. Type System & Data Structures

QCLang uses a strict type system. You can define custom types using aliases and structs.

### 2.1 Standard Types

* **Keywords**: `int`, `float`, `bool`, `string`, `qubit`, `qreg`, `cbit`.
* **Syntax**: `let name: type = value;`
* **Example**:
```rust
let count: int = 10;
let probability: float = 0.5;
let is_active: bool = true;

```



### 2.2 Tuples

* **Lexer Tokens**: `ParenOpen` (`(`), `Comma` (`,`), `ParenClose` (`)`).
* **Syntax**: `(type1, type2, ...)` and values as `(val1, val2, ...)`.
* **Example**:
```rust
// A tuple containing an int and a qubit
let pair: (int, qubit) = (1, |0>);

// Accessing tuple members (using .index)
let id: int = pair.0;

```



### 2.3 Type Aliases

* **Lexer Token**: `KwType` (`type`).
* **Syntax**: `type AliasName = TargetType;`
* **Example**:
```rust
type QuantumState = (qubit, qubit);
let my_state: QuantumState = (|0>, |1>);

```



### 2.4 Structs

* **Lexer Tokens**: `KwStruct` (`struct`), `BraceOpen` (`{`), `BraceClose` (`}`).
* **Syntax**:
* Definition: `struct Name { field: type, ... };`
* Initialization: `Name { field: value, ... }`


* **Example**:
```rust
// Definition
struct Experiment {
    id: int,
    target: qubit,
    description: string,
};

// Initialization
let my_exp: Experiment = Experiment {
    id: 101,
    target: |0>,
    description: "Bell state test",
};

// Accessing fields
let current_id: int = my_exp.id;

```



---

## 3. Declarations & Assignments

QCLang manages resources through `let` and `qreg` keywords.

### 3.1 Variables (`let`)

* **Immutable**: `let x: int = 5;` (Cannot be changed).
* **Mutable**: `let mut x: int = 5;` (Can be reassigned).
* **Quantum Warning**: Qubits **cannot** be mutable.

### 3.2 Quantum Registers (`qreg`)

* **Lexer Token**: `KwQreg` (`qreg`).
* **Syntax**: `qreg name[size] = |bits>;`
* **Example**:
```rust
// Initialize 4 qubits all to zero
qreg my_register[4] = |0000>;

```



---

## 4. Control Flow

Standard and quantum-specific control flow structures.

### 4.1 Conditionals (`if` / `qif`)

* **Classical**: Uses `bool` results.
* **Quantum**: Uses `qif` for operations conditioned on quantum contexts.
* **Example**:
```rust
if (x == 1) {
    // Classical logic
} else {
    // Alternative
}

```



### 4.2 Loops (`for` / `while`)

* **Range Loop**: `for var in range(start, end) { ... }`
* **Example**:
```rust
for i in range(0, 4) {
    H(q[i]); // Apply gate to each qubit in a register
}

```



---

## 5. Functions

Functions are defined with explicit return types using the `->` (Arrow) token.

* **Syntax**: `fn name(param: type) -> return_type { body }`
* **Example**:
```rust
fn prepare_qubit(id: int) -> qubit {
    let q: qubit = |0>;
    if (id == 1) {
        X(q);
    }
    return q;
}

```



---

## 6. Quantum Gate Syntax

Gates are treated as special function-like calls recognized by the compiler.

### 6.1 Unary Gates (1 Qubit)

* **Gates**: `H`, `X`, `Y`, `Z`, `T`, `S`.
* **Syntax**: `Gate(qubit);`
* **Example**: `H(q0);`

### 6.2 Binary Gates (2 Qubits)

* **Gates**: `CNOT`, `SWAP`.
* **Syntax**: `Gate(control, target);` or `Gate(q1, q2);`
* **Example**: `CNOT(q0, q1);`

### 6.3 Parametric Gates

* **Gates**: `RX`, `RY`, `RZ`.
* **Syntax**: `Gate(angle, qubit);`
* **Example**: `RX(3.14, q0);`

### 6.4 Measurement

* **Syntax**: `measure(qubit)` returns a `cbit`.
* **Example**:
```rust
let m: cbit = measure(q0);

```



---

## 7. Quantum Safety Rules (Affine Logic)

Because QCLang implements affine typing, the lexer and parser allow the following syntax, but the **Semantic Analyzer** will block it if rules are broken:

1. **No Re-use after Measurement**: You cannot use `q0` in a gate after calling `measure(q0)`.
2. **No Reassignment**: `q = H(q);` is invalid syntax for quantum types. Use `H(q);` instead.
3. **No Cloning**: You cannot do `let q2: qubit = q1;` and then use both; the original `q1` is consumed.