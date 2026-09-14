# Question: a write through the pointer field of a view returned by a call.
# Upstream (`1.1.0.dev2026082605`) runs it: `9`.
#
# Mojito rejects it: "expression must be mutable in assignment ('write
# through a Pointer whose origin mutability is not known here')". The
# binder's per-binding resolution reads a named binding's construction-time
# origins, and a call result has none. Before that check the program died
# in MIR ("selected subscript reference receiver has no retained caller
# place").
#
# On the fix: promote this file to
# `assets/ok/call_result_pointer_field_write.mojo` with its manifest rows,
# and delete the `call-result-pointer-field-write` ledger row in
# docs/roadmap.md.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def make(mut xs: List[Int]) -> P[origin_of(xs)]:
    return P(Pointer(to=xs))

def main():
    var xs = List[Int]()
    xs.append(7)
    make(xs).src[][0] = 9
    print(xs[0])
