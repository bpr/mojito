# PROBE: a static method's `Pointer[T, o]` result over a `Pointer(to=z)`
# argument keeps `z` alive for the result's later reads.
#
# **Differs.** The pin prints "8" then "9". Mojito's VM prints "8", then
# fails with "field access on non-struct None": `z` is destroyed right after
# `S.same(...)`, before `t[].v` reads through the result, while the instance
# method `s.isame(...)` keeps `y` alive. Filed in `docs/roadmap.md` R2288.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest
# rows.
#
# Observed 2026-11-17 against the pinned Mojo (mojo-2026111316).
#
# Run:    mojo run static_method_pointer_result_keeps_argument.mojo
struct Noisy(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


@fieldwise_init
struct S:
    @staticmethod
    def same[m: Bool, //, o2: Origin[mut=m]](p: Pointer[Noisy, o2]) -> Pointer[Noisy, o2]:
        return p

    def isame[m: Bool, //, o2: Origin[mut=m]](self, p: Pointer[Noisy, o2]) -> Pointer[Noisy, o2]:
        return p


def main():
    var y = Noisy(8)
    var s = S()
    var r = s.isame(Pointer(to=y))
    print(r[].v)
    var z = Noisy(9)
    var t = S.same(Pointer(to=z))
    print(t[].v)
