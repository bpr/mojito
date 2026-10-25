# Copies and destructions through clustered cases: a cluster's member runs
# its remaining tests before its bindings, so a `var` binding copies only
# once its case has matched, and a failed guard destroys the copy before
# the next member runs. A run led by a binding is not clustered, and its
# copies follow case order too. An `Optional` subject's discriminant reads
# it in place, so its enum-case tests copy nothing.
struct T(ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        print("  copy", copy.n)
        self.n = copy.n

    def __deinit__(deinit self):
        print("  del", self.n)


@fieldwise_init
struct W(ImplicitlyCopyable):
    var k: Int
    var t: T


def g(tag: String) -> Bool:
    print("  guard", tag)
    return False


def main():
    print("tuple members")
    var p = (2, T(1))
    match p:
        case (2, var a) if g("a"):
            print("a", a.n)
        case (3, var b):
            print("b", b.n)
        case (2, var c):
            print("c", c.n)
    print("field members")
    var w = W(3, T(4))
    match w:
        case W(k=2, t=var a):
            print("a", a.n)
        case W(k=3, t=var b) if g("b"):
            print("b", b.n)
        case W(k=3, t=var c):
            print("c", c.n)
    print("a leading binding")
    var q = (T(5), 2)
    match q:
        case (var a, 3):
            print("a", a.n)
        case (var b, 2):
            print("b", b.n)
        case _:
            print("other")
    print("optional cases")
    var o = Optional(T(6))
    match o:
        case .Some(_) if g("some"):
            print("some")
        case .Some(var a):
            print("a", a.n)
        case .None:
            print("none")
    match o:
        case .Some(ref r):
            print("r", r.n)
        case .None:
            print("none")
    if o:
        print("truthy", o.value().n)
    print("end")
