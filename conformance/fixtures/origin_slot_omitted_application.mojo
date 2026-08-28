# Both compilers reject an annotation that omits a struct's explicit
# origin slot where nothing infers it (`var p: Pair[Int] = ...`), with
# different diagnostics: the a79bdf59f2 pin reports "'Pair' failed to
# infer parameter 'o', specify the parameter or use '_' or '...'", Mojito
# reports a constructor-inference failure. NOTE Mojito's origin-argument
# compat rule remains MORE lenient than upstream in positions upstream
# cannot infer (alias bodies like `_TakeDictEntryIter[Self.K, Self.V]`
# still omit origin slots); the tightening follow-up is recorded in
# docs/roadmap.md.
@fieldwise_init
struct Pair[T: Copyable, o: Origin[mut=True]]:
    var src: Pointer[Self.T, Self.o]

def main():
    var n = 7
    var p: Pair[Int] = Pair(Pointer(to=n))
    print(p.src[])
