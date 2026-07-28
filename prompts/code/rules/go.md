#### Logic Errors
- Incorrect conditionals or missing else branches
- Loop variable mutations that affect iteration behaviour
- Off-by-one errors in slice bounds or index calculations
- Incorrect use of `nil` vs empty slice/map — `nil` slices range safely, `nil` maps panic on write

#### Error Handling
- Errors ignored with `_` instead of being checked (Go convention: check every error)
- Missing defer for resource cleanup (file close, mutex unlock)
- `panic` / `recover` used where error return would be appropriate
- Error values compared with `==` instead of using `errors.Is` / `errors.As`

#### Concurrency
- Goroutine leaks — goroutines launched without a clear termination path
- Channel closes without signalling all producers/consumers
- Missing `sync.WaitGroup` or `sync.Once` for coordination
- `sync.Mutex` copied by value (must be used through a pointer)
- Incorrect use of `context.Context` — not propagated through call chain

#### Performance
- Unnecessary allocations in hot paths — prefer `make` with capacity hint
- `fmt.Sprintf` in tight loops where `strings.Builder` would be better
- Reflection (`reflect`) used where generics or interfaces would suffice

#### Style & Idiom
- Variable shadowing that confuses intent
- Very long functions (>100 lines) that should be decomposed
- Package name doesn't match directory name
- Exported symbols without doc comments (Go convention)
