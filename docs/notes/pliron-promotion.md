# Pliron Backend Promotion

Decision recorded 2026-09-08: Pliron is Mojito's supported path to LLVM and
optimized native binaries. “Supported” describes the backend's status; the
`backend-pliron` feature remains optional so the default compiler and VM build
do not acquire LLVM dependencies.

The decision follows the Stage 6 evidence in `pliron-stage6.md`: sustained
VM/native differential and sanitizer parity, reproducible artifacts and Linux
toolchain discovery, accepted benchmark results, distributable bundles, and no
Mojito-maintained Pliron fork. The first dependency-upgrade rehearsal moved to
Pliron revision `477e6b0edb18b29df4cf7b90f0f468dc8a872f22`, whose LLVM layer
uses `llvm-sys 231`, against LLVM 23.1.0 via `LLVM_SYS_231_PREFIX`. The required
source changes were narrow upstream API adaptations and did not alter Mojito
semantics or its native ABI.

The upgrade gate ran all 325 selected repository tests. After it exposed and
we fixed an existing display-monomorphization ordering bug, the other 324 tests
were green and the repaired fixture passed the focused O0/release plus
ASan/LSan differential. The standalone spike gate passed 11/11 with clean
Clippy. The capability snapshot changed only for upstream DCE ordering; native
parity remains at 444 executable differentials, 34 error differentials, and
zero exclusions.

Promotion does not tighten Pliron's architectural integration. Verified,
serialized MIR remains the backend-independent waist, the register VM remains
the executable semantic oracle, and Pliron remains a removable lowering below
that waist. A future Cranelift backend remains feasible through the same MIR,
target/layout, and runtime-ABI contracts; it must not fork language semantics.

Upgrade policy is to pin an audited Pliron revision, run the Pliron gate and
its parity, sanitizer, reproducibility, distribution, and spike coverage, and
record intentional snapshot changes. A correctness regression disables the
affected optimization or native path until parity is restored; it is never
papered over with a silent VM fallback.
