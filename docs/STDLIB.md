# 📦 Mumei Standard Library Reference

## Overview

| Module | Auto-import | Description |
|---|---|---|
| `std/prelude.mm` | ✅ Yes | Traits, ADTs, collection interfaces |
| `std/alloc.mm` | ❌ `import "std/alloc"` | Dynamic memory, Vector, HashMap |
| `std/option.mm` | ❌ `import "std/option"` | `Option<T>` operations |
| `std/stack.mm` | ❌ `import "std/stack"` | Bounded stack operations |
| `std/result.mm` | ❌ `import "std/result"` | `Result<T, E>` operations |
| `std/list.mm` | ❌ `import "std/list"` | Recursive list ADT + Sort algorithms |
| `std/container/bounded_array.mm` | ❌ `import "std/container/bounded_array"` | Bounded array with sorted operations |
| `std/libc.mm` | ❌ `import "std/libc"` | Verified C standard library wrappers |
| `std/container/verified_vector.mm` | ❌ `import "std/container/verified_vector"` | Verified vector with quantifier-based contracts |

---

## std/prelude.mm (Auto-imported)

The prelude is automatically loaded by the compiler. No `import` statement needed.

### Traits

| Trait | Methods | Laws | Description |
|---|---|---|---|
| **Eq** | `eq(a, b) -> bool` | reflexive, symmetric | Equality |
| **Ord** | `leq(a, b) -> bool` | reflexive, transitive | Total ordering |
| **Numeric** | `add`, `sub`, `mul`, `div(b where v!=0)` | commutative_add | Arithmetic with zero-division prevention |
| **Sequential** | `seq_len(s) -> i64`, `seq_get(s, i) -> i64` | non_negative_length, bounds_safe | Abstract collection interface |
| **Hashable** | `hash(a) -> i64` | deterministic | Hash key constraint |
| **Owned** | `is_alive(a) -> bool`, `consume(a) -> Self` | alive_before_consume | Ownership tracking |

### ADTs

```mumei
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
enum List<T> { Nil, Cons(T, Self) }
struct Pair<T, U> { first: T, second: U }
```

### Prelude Atoms

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `prelude_is_some(opt)` | `opt >= 0 && opt <= 1` | `result >= 0 && result <= 1` | Check if Option is Some |
| `prelude_is_none(opt)` | `opt >= 0 && opt <= 1` | `result >= 0 && result <= 1` | Check if Option is None |
| `prelude_is_ok(res)` | `res >= 0 && res <= 1` | `result >= 0 && result <= 1` | Check if Result is Ok |

---

## std/alloc.mm — Dynamic Memory Management

```mumei
import "std/alloc" as alloc;
```

### Pointer Types

| Type | Definition | Description |
|---|---|---|
| `RawPtr` | `i64 where v >= 0` | Valid heap pointer |
| `NullablePtr` | `i64 where v >= -1` | Nullable pointer (-1 = null) |

### Vector\<T\>

```mumei
struct Vector<T> {
    ptr: i64 where v >= 0,   // heap pointer
    len: i64 where v >= 0,   // current element count
    cap: i64 where v > 0     // allocated capacity
}
```

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `alloc_raw(size)` | `size > 0` | `result >= -1` | Allocate heap memory |
| `dealloc_raw(ptr)` | `ptr >= 0` | `result >= 0` | Free heap memory |
| `vec_new(cap)` | `cap > 0` | `result >= 0` | Create empty vector |
| `vec_push(len, cap)` | `len >= 0 && cap > 0 && len < cap` | `result >= 0 && result <= cap && result == len + 1` | Push element (precise length) |
| `vec_get(len, index)` | `len > 0 && index >= 0 && index < len` | `result >= 0` | Get element (bounds-checked) |
| `vec_len(len)` | `len >= 0` | `result == len` | Get length |
| `vec_is_empty(len)` | `len >= 0` | `0 or 1` | Check if empty |
| `vec_grow(old, new)` | `old > 0 && new > old` | `result > old` | Grow capacity |
| `vec_drop(len, ptr)` | `len >= 0 && ptr >= 0` | `result >= 0` | Free vector |
| `vec_push_safe(len, cap)` | `len >= 0 && cap > 0` | `0=Ok, 1=Err` | Safe push with capacity check |
| `vec_set(len, index, value)` | `len > 0 && index >= 0 && index < len && value >= 0` | `result >= 0` | Set element (bounds-checked) |
| `vec_swap(len, i, j)` | `len > 0 && i >= 0 && i < len && j >= 0 && j < len` | `result >= 0` | Swap two elements (bounds-checked) |
| `vec_slice(len, start, end)` | `len >= 0 && start >= 0 && end >= start && end <= len` | `result >= 0 && result == end - start` | Slice (range-checked) |
| `vec_insert(len, cap, index)` | `len >= 0 && cap > 0 && len < cap && index >= 0 && index <= len` | `result >= 0 && result == len + 1` | Insert at index (bounds + capacity) |
| `vec_remove(len, index)` | `len > 0 && index >= 0 && index < len` | `result >= 0 && result == len - 1` | Remove at index (bounds-checked) |

### HashMap\<K, V\>

Key constraint: `K` must satisfy `Hashable + Eq` (defined in prelude).

```mumei
struct HashMap<K, V> {
    buckets: i64 where v >= 0,    // bucket array pointer
    size: i64 where v >= 0,       // current element count
    capacity: i64 where v > 0     // bucket count
}
```

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `map_new(capacity)` | `capacity > 0` | `result >= 0` | Create empty map |
| `map_insert(size, cap)` | `size >= 0 && cap > 0 && size < cap` | `result <= size + 1` | Insert key-value |
| `map_get(size, hash)` | `size >= 0 && hash >= 0` | `0=Ok, 1=Err` | Lookup by key hash |
| `map_contains_key(size, hash)` | `size >= 0 && hash >= 0` | `0 or 1` | Check key existence |
| `map_remove(size, hash)` | `size >= 0 && hash >= 0` | `result <= size` | Remove by key |
| `map_size(size)` | `size >= 0` | `result == size` | Get size |
| `map_is_empty(size)` | `size >= 0` | `0 or 1` | Check if empty |
| `map_rehash(old, new)` | `old > 0 && new > old` | `result > old` | Grow and rehash |
| `map_drop(size, buckets)` | `size >= 0 && buckets >= 0` | `result >= 0` | Free map |
| `map_insert_safe(size, cap)` | `size >= 0 && cap > 0` | `0=Ok, 1=Err` | Safe insert |
| `map_should_rehash(size, cap)` | `size >= 0 && cap > 0` | `0 or 1` | Load factor check (75%) |

---

## std/option.mm

```mumei
import "std/option" as option;
```

| Atom | Description |
|---|---|
| `is_some(opt)` | Returns 1 if Some, 0 if None |
| `is_none(opt)` | Returns 1 if None, 0 if Some |
| `unwrap_or(opt, default)` | Returns value or default |
| `map(opt, f)` | **Phase A**: Higher-order map via `atom_ref` — applies `f` to Some, returns 0 for None (`trusted`) |
| `map_apply(opt, default, mapped)` | Map (workaround): applies transformation (Some→mapped, None→default) — `@deprecated`, use `map` |
| `and_then_apply(opt, inner_opt)` | AndThen/FlatMap: chains Option-returning operations |
| `or_else(opt, alternative)` | OrElse: provides fallback Option |
| `filter(opt, condition)` | Filter: Some→None if condition is false |

---

## std/stack.mm

```mumei
import "std/stack" as stack;
```

```mumei
struct Stack<T> { top: i64 where v >= 0, max: i64 where v > 0 }
```

| Atom | Description |
|---|---|
| `stack_push(top, max)` | Push (requires `top < max`) |
| `stack_pop(top)` | Pop (requires `top > 0`) |
| `stack_is_empty(top)` | Check if empty |
| `stack_is_full(top, max)` | Check if full |
| `stack_clear(top)` | Clear with termination proof |

---

## std/result.mm

```mumei
import "std/result" as result;
```

| Atom | Description |
|---|---|
| `is_ok(res)` | Returns 1 if Ok, 0 if Err |
| `is_err(res)` | Returns 1 if Err, 0 if Ok |
| `unwrap_or_default(res, default)` | Returns value or default |
| `safe_divide(a, b)` | Division returning Result (Err on zero) |
| `result_map(res, f)` | **Phase A**: Higher-order map via `atom_ref` — applies `f` to Ok, returns 1 for Err (`trusted`) |
| `result_map_apply(res, default, mapped)` | Map (workaround): Ok→mapped, Err→default — `@deprecated`, use `result_map` |
| `result_and_then(res, inner_res)` | AndThen/FlatMap: chains Result operations |
| `result_or_else(res, alternative)` | OrElse: provides fallback on Err |
| `result_map_err(res, mapped_err)` | MapErr: transforms Err value |
| `result_wrap_err(res, err_code, offset)` | WrapErr: remap error code for package boundaries |
| `result_unwrap_or_else(res, ok_val, err_default)` | UnwrapOrElse: final error handling |
| `result_flatten(outer, inner)` | Flatten: `Result<Result<T,E>,E>` → `Result<T,E>` |

---

## std/list.mm

```mumei
import "std/list" as list;
```

```mumei
enum List { Nil, Cons(i64, Self) }
```

| Atom | Description |
|---|---|
| `is_empty(list)` | Check if Nil |
| `head_or(list, default)` | Get head or default |
| `is_sorted_pair(a, b)` | Check if a <= b |
| `insert_sorted(val, sorted_tag)` | Insert into sorted position |

### Immutable List Operations

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `list_head(list)` | `list ∈ {0,1}` | `result ∈ {0,1}` | Head as Option (Nil→None, Cons→Some) |
| `list_tail(list)` | `list ∈ {0,1}` | `result ∈ {0,1}` | Tail (new list, original unchanged) |
| `list_append(list, value)` | `list ∈ {0,1}` | `result == 1` | Append returns non-empty list |
| `list_prepend(list, value)` | `list ∈ {0,1}` | `result == 1` | Prepend (O(1), Cons construction) |
| `list_length(list)` | `list ∈ {0,1}` | `result >= 0` | Length (tag-based abstraction) |
| `list_reverse(list)` | `list ∈ {0,1}` | `result == list` | Reverse (tag preserved) |

### Higher-Order Fold / Map (Phase A)

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `fold_left(n, init, f)` | `n >= 0` | `result >= 0` | **Phase A**: Generic left fold via `atom_ref` — `f: atom_ref(i64, i64) -> i64` (`trusted`, body uses `arr[i]` stub) |
| `list_map(n, f)` | `n >= 0` | `result == n` | **Phase A**: Map via `atom_ref` — `f: atom_ref(i64) -> i64` (`trusted`, element count preserved) |

> **Warning**: `fold_left` body references `arr[i]` without an array parameter — do NOT run `mumei build std/list.mm` in isolation. Phase B will add proper array parameter support.

### Reduce / Fold Operations

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `fold_sum(n)` | `n >= 0` | `result >= 0` | Sum all elements |
| `fold_count_gte(n, threshold)` | `n >= 0` | `0 <= result <= n` | Count elements ≥ threshold |
| `fold_min_index(n)` | `n >= 0` | `-1 <= result < n` | Index of minimum element |
| `fold_max_index(n)` | `n >= 0` | `-1 <= result < n` | Index of maximum element |
| `fold_all_gte(n, threshold)` | `n >= 0` | `result ∈ {0,1}` | All elements ≥ threshold? (runtime forall) |
| `fold_any_gte(n, threshold)` | `n >= 0` | `result ∈ {0,1}` | Any element ≥ threshold? (runtime exists) |

### Sort Algorithms (Verified)

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `insertion_sort(n)` | `n >= 0` | `result == n` | Insertion sort with termination proof |
| `merge_sort(n)` | `n >= 0` | `result == n` | Merge sort with inductive invariant |
| `verified_insertion_sort(n)` | `n >= 0` | `result == n && forall(i, 0, result-1, arr[i] <= arr[i+1])` | Trusted: sorted output guarantee |
| `verified_merge_sort(n)` | `n >= 0` | `result == n && forall(i, 0, result-1, arr[i] <= arr[i+1])` | Trusted: sorted output guarantee |
| `binary_search(n, target)` | `n >= 0` | `result >= -1 && result < n` | Binary search with termination proof |
| `binary_search_sorted(n, target)` | `n >= 0 && forall(...)` | `result >= -1 && result < n` | Binary search with sorted precondition |

---

## std/container/verified\_vector.mm

```mumei
import "std/container/verified_vector" as vvec;
```

```mumei
struct VerifiedVector { len: i64 where v >= 0, cap: i64 where v > 0 }
```

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `vvec_sum(n)` | `n >= 0 && forall(arr[i] >= 0)` | `result >= 0` | Sum all non-negative elements |
| `vvec_all_bounded(n, upper)` | `n >= 0 && upper >= 0 && forall(0 <= arr[i] <= upper)` | `result == 1` | Check all elements within bound |
| `vvec_push_n(len, cap, count)` | `len >= 0 && cap > 0 && count >= 0 && len + count <= cap` | `result >= 0 && result == len + count` | Batch push with length guarantee |
| `vvec_range_check(len, start, end)` | `len > 0 && start >= 0 && end > start && end <= len` | `result == 1` | Validate index range |
| `vvec_binary_search(n, target)` | `n >= 0 && forall(sorted)` | `result >= -1 && result < n` | Binary search (sorted precondition) |

---

## std/container/bounded\_array.mm

```mumei
import "std/container/bounded_array" as bounded;
```

```mumei
struct BoundedArray { len: i64 where v >= 0, cap: i64 where v > 0 }
```

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `bounded_push(len, cap)` | `len >= 0 && cap > 0 && len < cap` | `result == len + 1` | Push with overflow prevention |
| `bounded_pop(len)` | `len > 0` | `result == len - 1` | Pop with underflow prevention |
| `bounded_is_empty(len)` | `len >= 0` | `0 or 1` | Check if empty |
| `bounded_is_full(len, cap)` | `len >= 0 && cap > 0` | `0 or 1` | Check if full |
| `sorted_identity(n)` | `n >= 0 && forall(sorted)` | `result == n && forall(sorted)` | Sorted invariant preservation |
| `sorted_insert_len(n, cap)` | `n >= 0 && cap > 0 && n < cap` | `result == n + 1` | Sorted insert (length tracking) |

---

## std/libc.mm — Verified C Library Wrappers

```mumei
import "std/libc" as libc;
```

Verified wrappers for C standard library functions via `extern "C"` FFI.
Each extern function declares strict `requires`/`ensures` contracts verified by Z3 at call sites.
Parameters use mumei's abstract size representation (i64) rather than raw pointers.

### Memory Operations

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `libc::safe_memcpy(dst_size, src_size, n)` | `n >= 0 && dst_size >= n && src_size >= n` | `result >= 0` | Copy n bytes (no overlap) |
| `libc::safe_memmove(dst_size, src_size, n)` | `n >= 0 && dst_size >= n && src_size >= n` | `result >= 0` | Move n bytes (overlap safe) |
| `libc::safe_memset(buf_size, value, n)` | `n >= 0 && buf_size >= n && value >= 0 && value <= 255` | `result >= 0` | Fill n bytes with value |

### String Operations

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `libc::safe_strlen(buf_size)` | `buf_size > 0` | `result >= 0 && result < buf_size` | Get string length (bounded) |
| `libc::safe_snprintf(buf_size, n)` | `buf_size > 0 && n > 0 && n <= buf_size` | `result >= 0` | Formatted print to buffer |

### Memory Allocation

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `libc::safe_malloc(size)` | `size > 0` | `result >= -1` | Allocate memory (-1 = failure) |
| `libc::safe_calloc(count, size)` | `count > 0 && size > 0` | `result >= -1` | Allocate zeroed memory (-1 = failure) |
| `libc::safe_realloc(ptr, old_size, new_size)` | `ptr >= 0 && old_size >= 0 && new_size > 0` | `result >= -1` | Reallocate memory (-1 = failure) |
| `libc::safe_free(ptr)` | `ptr >= 0` | `result >= 0` | Free allocated memory |

### C Header Generation

`mumei build std/libc.mm --emit c-header` generates a `.h` file with Doxygen `@pre`/`@post` annotations:

```c
/** @brief safe_memcpy */
/** @pre n >= 0 && dst_size >= n && src_size >= n */
/** @post result >= 0 */
int64_t safe_memcpy(int64_t dst_size, int64_t src_size, int64_t n);
```

---

## std/json.mm — JSON Operations

```mumei
import "std/json" as json;
```

FFI-backed standard library for JSON parsing and generation.
Wraps Rust `serde_json` behind a handle-based API.

### Parse / Stringify

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `json::parse(input)` | `true` | `result >= 0` | Parse JSON string and return a handle |
| `json::stringify(handle)` | `handle >= 0` | `true` | Convert JSON handle to string |

### Value Access

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `json::get(handle, key)` | `handle >= 0` | `result >= 0` | Get value from object by key |
| `json::get_int(handle, key)` | `handle >= 0` | `true` | Get integer value |
| `json::get_str(handle, key)` | `handle >= 0` | `true` | Get string value |
| `json::get_bool(handle, key)` | `handle >= 0` | `result in {0,1}` | Get boolean value |

### Array Operations

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `json::array_len(handle)` | `handle >= 0` | `result >= 0` | Get array length |
| `json::array_get(handle, index)` | `handle >= 0 && index >= 0` | `result >= 0` | Get array element |
| `json::array_new()` | `true` | `result >= 0` | Create empty array |
| `json::array_push(handle, value)` | `handle >= 0` | `result >= 0` | Append value to array |

### Type Checks

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `json::is_null(handle)` | `handle >= 0` | `result in {0,1}` | Check if null |
| `json::is_object(handle)` | `handle >= 0` | `result in {0,1}` | Check if object |
| `json::is_array(handle)` | `handle >= 0` | `result in {0,1}` | Check if array |

### Value Construction

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `json::object_new()` | `true` | `result >= 0` | Create empty object |
| `json::object_set(handle, key, value)` | `handle >= 0` | `result >= 0` | Set key-value pair on object |
| `json::from_int(value)` | `true` | `result >= 0` | Create JSON value from integer |
| `json::from_str(value)` | `true` | `result >= 0` | Create JSON value from string |
| `json::from_bool(value)` | `value in {0,1}` | `result >= 0` | Create JSON value from boolean |

### Memory Management (Plan 16)

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `json::free(handle)` | `handle >= 0` | `result in {0,1}` | Release JSON handle (1=success, 0=invalid) |
| `json::str_free(handle)` | `handle >= 0` | `result in {0,1}` | Release string handle (1=success, 0=invalid) |

---

## std/http.mm — HTTP Client

```mumei
import "std/http" as http;
```

HTTP client wrapping Rust `reqwest` via FFI. Provides a handle-based API.
Can be combined with `task_group` for parallel requests.

### Requests

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `http::get(url)` | `true` | `result >= 0` | HTTP GET request |
| `http::post(url, body)` | `true` | `result >= 0` | HTTP POST request |
| `http::put(url, body)` | `true` | `result >= 0` | HTTP PUT request |
| `http::delete(url)` | `true` | `result >= 0` | HTTP DELETE request |

### Response

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `http::status(handle)` | `handle >= 0` | `result >= 0` | Get status code (200, 404, etc.) |
| `http::body(handle)` | `handle >= 0` | `result >= 0` | Get response body (string handle) |
| `http::body_json(handle)` | `handle >= 0` | `result >= 0` | Parse response body as JSON |
| `http::is_ok(handle)` | `handle >= 0` | `result in {0,1}` | Check success (2xx) |
| `http::is_error(handle)` | `handle >= 0` | `result in {0,1}` | Check error |

### Headers

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `http::header_get(handle, name)` | `handle >= 0` | `result >= 0` | Get header value |
| `http::header_set(handle, name, value)` | `handle >= 0` | `result >= 0` | Set header value |

### Memory Management (Plan 16)

| Atom | Requires | Ensures | Description |
|---|---|---|---|
| `http::free(handle)` | `handle >= 0` | `result in {0,1}` | Release HTTP response handle (1=success, 0=invalid) |

---

## Path Resolution

The resolver searches for `std/` imports in order:

1. **Project root** — `base_dir/std/option.mm`
2. **Compiler binary directory** — alongside `mumei` executable
3. **Current working directory**
4. **`CARGO_MANIFEST_DIR`** — for development builds
5. **`MUMEI_STD_PATH`** — custom installation path
