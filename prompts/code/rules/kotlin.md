#### Logic Errors
- Incorrect conditionals — null checks missing on nullable types
- Incorrect use of `!!` operator (forces null to throw)
- Confusion between nullable (`Type?`) and non-nullable (`Type`) types
- Incorrect scope function usage (`let` vs `run` vs `apply` vs `also` vs `with`)

#### Coroutines
- `GlobalScope.launch` used without clear cancellation management
- Blocking calls (`Thread.sleep()`, `runBlocking`) inside a coroutine context
- Missing `Dispatchers.Main.immediate` on UI updates when already on main thread
- `async` / `await` used where `flow` would be more appropriate for streaming data

#### Null Safety
- `!!` used where safe cast or Elvis operator would be safer
- Platform types from Java interop not resolved to Kotlin types
- Incorrect initialisation of `lateinit` vars — accessed before assignment

#### Performance
- Unnecessary object allocations in hot paths — use `inline` functions or `value` classes
- Reflection used where generics or reified type parameters would work
- Sequence processing (`asSequence()`) not terminated with terminal operation
