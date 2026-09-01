# A view holding `Pointer(to=n)` of a `ref` parameter, returned from the
# constructing function: the call lends its `ref`-bound source to the
# pointer-carrying result exactly as it lends a ref-field view's source, so
# `n` stays alive while `v` does and the returned handle re-roots at it.
# Both compilers print 7 (pin a79fbdf59f2, 2026-09-01).
@fieldwise_init
struct View[o: Origin[mut=False]]:
    var src: Pointer[Int, Self.o]

def make(ref n: Int) -> View[origin_of(n)]:
    return View(Pointer(to=n))

def main():
    var n = 7
    var v = make(n)
    print(v.src[])
