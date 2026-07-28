#### Logic Errors
- Incorrect if-condition logic or missing else branches
- Off-by-one errors in loops, slices, or ranges
- Incorrect use of `Result` / `Option` — unwrapping without handling the error case
- Missing or incorrect `match` arms, especially for enums that may gain variants later
- Incorrect use of `unsafe` — verify safety invariants are documented and upheld

#### Ownership & Borrowing
- Unnecessary clones of large types (use references where possible)
- Lifetime annotations that are overly restrictive or incorrect
- `Rc`/`Arc` used where `&` would suffice
- Interior mutability (`RefCell`, `Mutex`) used without clear justification
- Dropping a `MutexGuard` or `RwLockReadGuard` before the critical section ends

#### Concurrency
- Shared state without synchronisation (missing `Arc<Mutex<T>>` or channels)
- Incorrect use of `async` — blocking calls inside async functions without `spawn_blocking`
- Missing `Send + Sync` bounds on types passed across threads
- Deadlock risk from inconsistent lock ordering

#### Error Handling
- Errors swallowed with `.ok()` or `.unwrap()` without justification
- Custom error types that don't implement `std::error::Error` properly
- Panic-prone operations (`unwrap`, `expect`, index without bounds check) in library code
- Incorrect error propagation — returning the wrong error variant

#### Performance
- Large allocations inside hot loops
- `Vec` resized repeatedly without `with_capacity`
- `String` concatenation with `+` in loops (use `join` or `format!`)
- `HashMap` / `HashSet` used where `BTreeMap` / `BTreeSet` would be more appropriate
