# PROBE: a store through a dereferenced pointer whose pointee type carries
# a loan.
#
# **Differs.** The pin prints 5. Mojito fails on the VM with "use after
# Pointer deallocation". Filed in `docs/roadmap.md` §3. When Mojito runs it,
# promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against the pinned Mojo.
#
# Run:    mojo run pointer_deref_store_loan_carrying_pointee.mojo
from std.memory.alloc import unsafe_alloc

def main():
    var xs: List[Int] = [4, 5, 6]
    var q = unsafe_alloc[Span[Int, origin_of(xs)]](1)
    q[] = Span(xs)
    print(q[0][1])
    q.free()
