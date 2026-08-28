# Recorded acceptance divergence (mojito-only, 2026-08-27): a struct FIELD
# naming a generic struct with all explicit parameters omitted
# (`var inner: Holder`). Mojito accepts — parameters infer from the
# fieldwise constructor argument, loan-sound — and prints 7; the
# a79bdf59f2 pin rejects ("'Holder[_]' is not concrete, use '[]' to bind
# missing parameters"). Tightening follow-up recorded in docs/roadmap.md;
# stdlib spellings stay fully applied.
@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Int, Self.o]

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var inner: Holder

def main():
    var n = 7
    var h = Holder(Pointer(to=n))
    var w = Wrap(h^)
    print(w.inner.src[])
