# Question: reassigning a `mut` parameter as a whole (the same holds for
# `self = other^` in a `mut self` method).
# Upstream (`1.1.0.dev2026082605`) destroys the caller's old value at the
# assignment: `del 1`, `del 2`, then `7`, `del 7`, `del 8`.
#
# Mojito today never destroys Inner 1 and Inner 2 on the VM: `7`, `del 7`,
# `del 8`. A local's whole reassignment does destroy the old value; the
# `mut` parameter's `DefVar` writes through the caller's slot without it.
#
# On the fix: promote this file to
# `assets/ok/mut_parameter_reassignment_drop.mojo` with its manifest rows,
# and delete the `mut-parameter-reassignment-drop` ledger row in
# docs/roadmap.md.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def reset(mut p: Pair):
    p = Pair(Inner(7), Inner(8))

def main():
    var p = Pair(Inner(1), Inner(2))
    reset(p)
    print(p.a.id)
