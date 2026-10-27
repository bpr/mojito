# A bracket after a function's name binds the function's parameters,
# whatever the operands' spelling: the subscript is classified by what its
# base denotes. The value names the function's instance at those bindings,
# so a value parameter no runtime argument spells (`twice[4]`) reaches the
# body, and a keyword operand binds the parameter it names.

@fieldwise_init
struct Point(ImplicitlyCopyable, Writable):
    var x: Int


def twice[n: Int]() -> Int:
    return n * 2


def pair[a: Int, b: Int]() -> Int:
    return a * 10 + b


def total[*values: Int]() -> Int:
    var sum = 0
    comptime for value in values:
        sum += value
    return sum


def ident[T: ImplicitlyCopyable](x: T) -> T:
    return x


def pick[A: ImplicitlyCopyable, B: ImplicitlyCopyable](a: A, b: B) -> A:
    return a


def show[T: Writable & ImplicitlyCopyable](x: T) -> String:
    return String(x)


def apply(f: def() thin -> Int) -> Int:
    return f()


def main():
    var f = twice[4]
    print(f())
    var g = pair[1, 2]
    print(g())
    var h = pair[b=3, a=4]
    print(h())
    var t = total[1, 2, 3]
    print(t())
    print(apply(total[4, 5]))
    var i = ident[String]
    print(i("a"))
    var j = ident[Point]
    print(j(Point(3)).x)
    var k = ident[T=Point]
    print(k(Point(4)).x)
    var p = pick[Point, String]
    print(p(Point(5), "b").x)
    var s = show[Point]
    print(s(Point(6)))
    print(ident[Point](Point(7)).x)
