# A `#kgen.bind_params` binds a generator's whole declared list, origin
# binders and their `mut=` binders in place, as at the pin: a callable
# parameter's bracket, a generic alias, a struct's and a trait's
# parameterized value member, and a function value whose body reads its
# origin's `mut=` binder.
comptime Twice[o: ImmOrigin, n: Int] = n * 2
comptime MutOf[mut: Bool, //, o: Origin[mut=mut]] = mut


trait HasTwice:
    comptime twice[o: ImmOrigin, n: Int]: Int


struct S(HasTwice):
    comptime twice[o: ImmOrigin, n: Int]: Int = n * 3


def first[o: ImmOrigin, n: Int](p: Pointer[Int, o]) -> Int:
    return p[] + n


def is_mut[mut: Bool, //, o: Origin[mut=mut]](x: Pointer[Int, o]) -> Bool:
    return mut


def bound[f: def[o: ImmOrigin, n: Int](Pointer[Int, o]) thin -> Int](x: Int) -> Int:
    var y = x
    comptime g = f[ImmOrigin(origin_of(y)), 3]
    return g(Pointer(to=y))


def through[T: HasTwice]() -> Int:
    return T.twice[ImmStaticOrigin, 5]


def read_only(x: Int) -> Bool:
    var y = x
    var f = is_mut[o=ImmOrigin(origin_of(y))]
    return f(Pointer(to=y))


def main():
    print(Twice[ImmStaticOrigin, 3])
    comptime half = Twice[_, 4]
    print(half[ImmStaticOrigin])
    print(MutOf[o=MutAnyOrigin])
    print(S.twice[ImmStaticOrigin, 4], through[S]())
    print(bound[first](4))
    var a = 1
    var f = is_mut[o=origin_of(a)]
    print(f(Pointer(to=a)), read_only(a))
