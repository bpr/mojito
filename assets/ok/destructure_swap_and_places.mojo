# A destructuring assignment evaluates its right-hand side once, after the
# destinations: a swap reads both names before either is written, a tuple
# target of struct-field places stores a call's result, and a name target
# beside a place target writes the place with the name's old value.
@fieldwise_init
struct Point(ImplicitlyCopyable):
    var x: Int
    var y: Int


def pair() -> Tuple[Int, Int]:
    print("pair")
    return (10, 20)


def main():
    var a = 1
    var b = 2
    (a, b) = (b, a)
    print(a, b)
    var p = Point(0, 0)
    (p.x, p.y) = pair()
    print(p.x, p.y)
    var i = 0
    var q = Point(0, 0)
    (i, q.x) = (2, 7)
    print(i, q.x)
    (a, (p.x, b)) = (3, (4, 5))
    print(a, p.x, b)
