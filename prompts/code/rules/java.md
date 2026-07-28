#### Logic Errors
- Incorrect conditionals or null checks — objects may be null at runtime
- Off-by-one errors in loop bounds or substring ranges
- Missing `break` in switch cases (fall-through without `// fall through` comment)
- Integer overflow — Java doesn't have unsigned types; use `Math.addExact` for sensitive arithmetic

#### Error Handling
- Exceptions caught but not logged or re-thrown (empty catch blocks)
- Checked exceptions swallowed or incorrectly propagated
- Resource leaks — streams, connections, or sessions not closed in `finally` or try-with-resources
- Null returned from methods that document non-null contract

#### Concurrency
- Shared mutable state without synchronisation
- `HashMap` used in multi-threaded context without `ConcurrentHashMap`
- Incorrect use of `volatile` — doesn't guarantee atomicity for compound operations
- Deadlock risk from nested synchronised blocks with inconsistent ordering

#### Performance
- String concatenation in loops — use `StringBuilder` or `StringBuffer`
- Boxing/unboxing overhead in hot paths (prefer primitives)
- Inefficient collection usage — `List.contains` on `ArrayList` vs `HashSet`
- Large object graphs serialised/deserialised repeatedly

#### API Design
- Public methods missing `@Nullable` / `@NonNull` annotations for clarity
- Method signatures that accept and return null unnecessarily
- Overly broad exception types in method signatures
- Deprecated API usage without migration plan
