# A type parameter is refined by its scope's `where` clauses and `comptime
# if` conditions, as upstream's `maybeRefineParamType` merges the declared
# bound with the traits the assumptions prove. A requirement only the
# refinement declares is a call like any other: its argument's owner
# `b` ends before the call, as it does under a declared bound. So does a
# requirement an associated projection's declaring trait bounds.


trait Sh:
    def sh(self, t: Int):
        ...


trait Ix:
    def __getitem__(self, i: Int) -> Int:
        ...


trait Holder:
    comptime Elem: Sh & Copyable & Deinitable

    def first(self) -> Self.Elem:
        ...


struct B:
    var t: Int

    def __init__(out self, t: Int):
        self.t = t

    def __deinit__(deinit self):
        print("del", self.t)


@fieldwise_init
struct S(Copyable, Ix, Sh):
    var x: Int

    def sh(self, t: Int):
        print("sh", t)

    def __getitem__(self, i: Int) -> Int:
        print("get", i)
        return i


@fieldwise_init
struct H(Copyable, Holder):
    comptime Elem = S
    var s: S

    def first(self) -> Self.Elem:
        return self.s.copy()


def under_where[T: Copyable](s: T) where conforms_to(T, Sh):
    var b = B(1)
    s.sh(b.t)


def under_comptime_if[T: Copyable](s: T):
    comptime if conforms_to(T, Sh):
        var b = B(2)
        s.sh(b.t)


def subscript_under_where[T: Copyable](s: T) where conforms_to(T, Ix):
    var b = B(3)
    _ = s[b.t]


def through_projection[C: Holder](h: C):
    var e = h.first()
    var b = B(4)
    e.sh(b.t)


def ascending[T: ImplicitlyCopyable & Deinitable](
    a: T, b: T, c: T
) -> Bool where conforms_to(T, Comparable):
    return a < b < c


def main():
    under_where(S(0))
    under_comptime_if(S(0))
    subscript_under_where(S(0))
    through_projection(H(S(0)))
    print(ascending(1, 2, 3), ascending(3, 2, 1))
