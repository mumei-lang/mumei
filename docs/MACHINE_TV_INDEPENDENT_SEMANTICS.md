# Machine-TV independent semantics pilot

This pilot is separate evidence from the Machine-TV CPU differential tests.

- **CPU differential testing** samples concrete states and compares the lifter's model with this host CPU. It is strong bug detection evidence, but not a mathematical proof over all inputs.
- **Independent semantics pilot** proves one narrow state transition with Z3 over symbolic 64-bit inputs. The reference transition is written separately from the lifter implementation.

## Current proved slice

Instruction form: `addq %rsi, %rdi` (AT&T source, destination order).

Proved state items:

- destination register `%rdi` after execution;
- arithmetic result value used for flag modeling;
- carry flag (CF);
- overflow flag (OF).

Explicitly excluded state items:

- AF, PF and every flag other than CF/OF;
- memory contents, stack behavior, control flow and instruction bytes;
- every register other than the destination result item above;
- x86-64 forms other than the single register/register form;
- correctness of the CPU, assembler, disassembler, LLVM or Z3 itself.

The pilot lives in `mumei-emit-llvm/src/machine_tv_cpu_differential.rs` as
`addq_register_form_matches_independent_reference_semantics`. Although the proof
itself is symbolic Z3 reasoning, the test currently shares the CPU-differential
probe harness and therefore compiles only on x86-64 hosts. On another host it is
not executed and must not be reported as proved. An unsupported instruction or
an unmodeled flag is excluded rather than counted as proved.
