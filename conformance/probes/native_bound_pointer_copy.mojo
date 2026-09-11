# Question: copying a bound `Pointer` local — into another local, an
# annotated binding, or a constructor argument. Upstream
# (`1.1.0.dev2026082605`) and the Mojito VM print `5` three times.
#
# Mojito today (native): prints `q`'s own address for each read. MIR lowers
# the copy as a handle to `q`'s slot, and pliron dereferences that handle.
#
# On the fix: pass bound pointers in `assets/ok/pointer_origin_constructors.mojo`
# and `assets/ok/pointer_immutable_conversion.mojo`, and drop this probe.
@fieldwise_init
struct Plain[o: Origin]:
    var p: Pointer[Int, Self.o]

    def get(self) -> Int:
        return self.p[]


def main():
    var x = 5
    var q = Pointer(to=x)
    var t = q
    print(t[])
    var s: Pointer[Int, origin_of(x)] = q
    print(s[])
    var e = Plain(q)
    print(e.get())
