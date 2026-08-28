# Both compilers reject an annotation that omits a struct's explicit
# origin slot where nothing infers it (`var p: Pair[Int] = ...`): the
# a79bdf59f2 pin reports "'Pair' failed to infer parameter 'o', specify
# the parameter or use '_' or '...'", and Mojito's storage-annotation
# concreteness rule (tightened 2026-08-28) reports "'Pair' failed to
# infer parameter 'o'; specify the parameter explicitly" (Mojito has no
# `_`/`...` origin placeholders — a recorded subset gap). Constructor
# expressions with value arguments remain the origin-inference context
# on both compilers.
@fieldwise_init
struct Pair[T: Copyable, o: Origin[mut=True]]:
    var src: Pointer[Self.T, Self.o]

def main():
    var n = 7
    var p: Pair[Int] = Pair(Pointer(to=n))
    print(p.src[])
