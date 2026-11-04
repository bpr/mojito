# A bracket list may leave parameters unbound: a `_` leaves its own slot,
# with no default, and a trailing `...` every slot left, defaulted ones
# included. The application is then the generator over the open slots,
# which a generator-typed parameter takes and a later application binds.
comptime Add[a: Int, b: Int] = a + b
comptime Add3[a: Int, b: Int, c: Int] = a + b * c
comptime AddD[a: Int, b: Int = 10] = a + b


def apply[G: __generator_type[b: Int] Int]() -> Int:
    return G[3]


def apply2[G: __generator_type[a: Int, b: Int] Int]() -> Int:
    return G[3, 4]


def apply1[G: __generator_type[x: Int] Int]() -> Int:
    return G[5]


def first_open[G: __generator_type[a: Int, b: Int] Int]() -> Int:
    comptime H = G[1, _]
    return H[2]


def shifted[n: Int]() -> Int:
    return apply[Add[n, _]]()


trait HasGet:
    comptime get[i: Int, j: Int]: Int


struct Digits(HasGet):
    comptime get[i: Int, j: Int]: Int = i * 10 + j


def through[T: HasGet]() -> Int:
    comptime g = T.get[4, _]
    return g[2]


def main():
    print(apply[Add[10, _]](), apply[Add[b=1, ...]]())
    comptime P = Add[100, _]
    print(P[1])
    print(apply2[Add[_, _]](), apply2[Add[...]](), apply1[Add[a=_, b=5]]())
    print(
        apply1[Add3[1, _, 3]](),
        apply2[Add3[_, 2, ...]](),
        apply2[Add3[1, ...]](),
        apply2[Add3[b=2, ...]](),
    )
    comptime Both = AddD[...]
    comptime Rest = AddD[1, ...]
    comptime Hole = AddD[_]
    print(Both[1, 2], Rest[5], Hole[5])
    print(apply1[Digits.get[1, _]](), through[Digits](), first_open[Add]())
    comptime g = Add
    comptime h = g[_, 5]
    print(h[1], shifted[7]())
