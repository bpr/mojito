# expect: violated constraint
# A `where` clause over an origin's `mut=` binder reads the mutability the
# call binds it to: a pointer to a read-only argument binds `False`, which
# violates `where mut`, as at the pin.
def only_mutable[
    mut: Bool, //, o: Origin[mut=mut]
](x: Pointer[Int, o]) -> Int where mut:
    return x[]


def read_only(x: Int) -> Int:
    return only_mutable(Pointer(to=x))


def main():
    var a = 1
    print(read_only(a))
