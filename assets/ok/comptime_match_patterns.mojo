# The pattern forms `comptime match` runs, each a compile-time condition
# over its subject: value parameters with literals, `|`, and a guarded
# binding; a generic tuple with a guard over its bindings; a concluding
# `Bool` match whose cases return; a type binding, `== Int as U`, and a
# `conforms_to` guard; a CTFE subject, demanded once; string and float
# literals; a match no case takes; a merging `|`; nesting in `comptime for`;
# `Self.n`; an enum case with its payload; a struct pattern; a fixed-length
# sequence; and `== m` against a compile-time binding.
def value[n: Int]() -> String:
    comptime match n:
        case 0:
            return "zero"
        case 1 | 2:
            return "small"
        case k if k > 100:
            return "big"
        case _:
            return "mid"


def pair[a: Int, b: Int]() -> String:
    comptime match (a, b):
        case (1, 2):
            return "one-two"
        case (1, _):
            return "one-any"
        case (x, y) if x == y:
            return "same"
        case _:
            return "other"


def boolean[b: Bool]() -> Int:
    comptime match b:
        case True:
            return 1
        case False:
            return 2


def bind_type[T: Copyable & Defaultable]():
    comptime match T:
        case U:
            comptime V = U
            print("bound")


def as_type[T: AnyType]():
    comptime match T:
        case == Int as U:
            print("as")
        case _:
            print("no")


def copyable[T: AnyType]():
    comptime match T:
        case U if conforms_to(U, Copyable):
            print("copyable")
        case _:
            print("not copyable")


def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


struct S[n: Int]:
    @staticmethod
    def name() -> String:
        comptime match Self.n:
            case 1:
                return "one"
            case _:
                return "many"


@fieldwise_init
struct P(Copyable, ImplicitlyCopyable):
    var a: Int
    var b: Int


def main():
    print(value[0](), value[2](), value[500](), value[50]())
    print(pair[1, 2](), pair[1, 5](), pair[3, 3](), pair[3, 4]())
    print(boolean[True](), boolean[False]())
    bind_type[Int]()
    as_type[Int]()
    as_type[Bool]()
    copyable[Int]()
    comptime match fib(10):
        case 55:
            print("fib55")
        case _:
            print("fibother")
    comptime s = "a"
    comptime match s:
        case "a":
            print("str a")
        case _:
            print("str other")
    comptime f = 0.5
    comptime match f:
        case 0.5:
            print("half")
        case _:
            print("other")
    comptime match 7:
        case 1:
            print("none")
    print("fell")
    comptime t = (5, 1)
    comptime match t:
        case (1, x) | (x, 1):
            print("x", x)
        case _:
            print("other")
    comptime for i in range(4):
        comptime match i:
            case 0 | 2:
                print("even", i)
            case j:
                print("odd", j)
    print(S[1].name(), S[2].name())
    comptime o = Optional[Int](3)
    comptime match o:
        case .Some(x):
            print("some", x)
        case .None:
            print("none")
    comptime p = P(1, 2)
    comptime match p:
        case P(a=1, b=y):
            print("p", y)
        case _:
            print("other")
    comptime l = [1, 2]
    comptime match l:
        case [a, b]:
            print(a, b)
        case _:
            print("other")
    comptime m = 4
    comptime match 4:
        case == m:
            print("eq m")
        case _:
            print("ne m")
