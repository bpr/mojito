# A mutable-origin `Pointer` converts to the `ImmOrigin(o)` pointer of the
# same provenance (upstream's `@implicit` capability-dropping constructor),
# in an annotated binding of each place's pointer.
def main():
    var x = 5
    var r: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    print(r[])
    var y = 6
    var s: Pointer[Int, ImmOrigin(origin_of(y))] = Pointer(to=y)
    print(r[] + s[])
