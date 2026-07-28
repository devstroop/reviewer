#### Memory Safety
- Buffer overflows or out-of-bounds array access
- Use-after-free or double-free — pointers dereferenced after `free()` or `delete`
- Incorrect `malloc` / `calloc` size calculations
- Missing null checks after `malloc` (can return NULL on failure)

#### Undefined Behaviour
- Signed integer overflow
- Shift operations with count >= type width
- Violating strict aliasing rules (casting between incompatible pointer types)
- Modifying a string literal (undefined behaviour)

#### Concurrency
- Data races on shared variables without `atomic` or mutex protection
- Incorrect use of `volatile` — not suitable for synchronisation
- Deadlock from inconsistent mutex lock ordering
- Signal handlers that call non-async-signal-safe functions

#### Resource Management
- File descriptors, sockets, or handles not closed on error paths
- Memory allocated in one layer and freed in another (ownership not documented)
- Incorrect use of RAII in C++ — destructors that can throw

#### C++-Specific
- Missing `virtual` destructor in polymorphic base classes
- Incorrect use of `std::move` on const objects (degenerates to copy)
- `auto_ptr` used instead of `unique_ptr` (deprecated in C++17, removed in C++26)
- Template metaprogramming that could be simplified with `constexpr` or `concepts`
- Unchecked `static_cast` / `reinterpret_cast` / `const_cast`
