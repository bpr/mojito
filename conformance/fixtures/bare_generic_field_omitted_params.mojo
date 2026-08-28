# Both compilers reject a struct FIELD naming a generic struct with all
# explicit parameters omitted (`var inner: Holder`): the a79bdf59f2 pin
# reports "'Holder[_]' is not concrete, use '[]' to bind missing
# parameters", and Mojito's storage-annotation concreteness rule
# (tightened 2026-08-28) mirrors it — "'Holder[_]' is not concrete; use
# '[]' to bind missing parameters".
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
