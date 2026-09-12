# A mutable-origin `Pointer` converts to the `ImmOrigin(o)` pointer of the
# same provenance (upstream's `@implicit` capability-dropping constructor),
# in an annotated binding from a temporary and from a bound pointer.
def main():
    var x = 5
    var r: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    print(r[])
    var y = 6
    var q = Pointer(to=y)
    var s: Pointer[Int, ImmOrigin(origin_of(y))] = q
    print(r[] + s[])
