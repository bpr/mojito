# Question: assigning to a `mut` `Pointer` parameter inside the callee.
# Upstream (`1.1.0.dev2026082605`) accepts it and prints `5`.
#
# Mojito today (both backends): MIR verification rejects the store
# (`WriteRef value type Pointer[Int, origin#0] is incompatible with
# referent Int`). The store lowers as a `WriteRef` through the parameter's
# slot handle, which is typed as the pointer itself, so the verifier
# expects a pointee-typed value there. Copying the parameter (`var c = p`)
# verifies and runs.
#
# On the fix: promote this file to
# `assets/ok/mut_pointer_parameter_reassign.mojo` with its manifest rows,
# and delete the `mut-pointer-parameter-reassignment` ledger row in
# docs/roadmap.md.
def write_back[o: Origin](mut p: Pointer[Int, o]):
    p = p


def main():
    var x = 5
    var q = Pointer(to=x)
    write_back(q)
    print(q[])
