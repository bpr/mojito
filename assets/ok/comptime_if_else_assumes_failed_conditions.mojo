# A compile-time arm knows that every earlier condition in its chain failed,
# as upstream's `buildBranchAssumption` gives each arm the negation of the
# earlier arms' conditions: an `else` after `not conforms_to(T, Writable)`,
# an `elif` after `not conforms_to(...) or ...`, a `comptime match` case
# after a guard that negates the conformance, and a parameter-valued
# conditional expression's else branch may each use `T` as `Writable`, and
# an `else` after `not (n > 0)` proves a callee's `where n > 0`.
@fieldwise_init
struct P(ImplicitlyCopyable):
    var v: Int


def show[T: AnyType](x: T):
    comptime if not conforms_to(T, Writable):
        print("opaque")
    else:
        print(x)


def chain[T: AnyType](x: T):
    comptime if conforms_to(T, Copyable) and False:
        print("unreached")
    elif not conforms_to(T, Writable) or False:
        print("opaque")
    elif True:
        print(x)


def matched[T: AnyType](x: T):
    comptime match T:
        case U if not conforms_to(U, Writable):
            print("opaque")
        case _:
            print(x)


def text[T: ImplicitlyCopyable](x: T) -> String:
    return String("opaque") if not conforms_to(T, Writable) else String(x)


def positive[n: Int]() where n > 0:
    print("positive", n)


def sign[n: Int]():
    comptime if not (n > 0):
        print("not positive", n)
    else:
        positive[n]()


def main():
    show(1)
    show(P(2))
    chain("three")
    chain(P(4))
    matched(5.5)
    matched(P(6))
    print(text(7))
    print(text(P(8)))
    sign[9]()
    sign[0]()
