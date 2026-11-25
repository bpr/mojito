# A type parameter an immutable pointer binds takes a later mutable pointer
# to the same storage, which converts to the immutable one, as the pin binds
# a parameter at its first argument (upstream prints `7 7`). The other order
# is the pin's "cannot be converted" rejection.
def last[T: ImplicitlyCopyable](a: T, b: T) -> T:
    return b


def main():
    var x = 5
    var p = Pointer(to=x)
    var q: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    var r = last(q, p)
    p[] = 7
    print(r[], q[])
