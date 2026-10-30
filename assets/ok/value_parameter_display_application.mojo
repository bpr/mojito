# A tuple or list display in a parameter position is its literal
# constructor's application, symbolic or closed: `(n, 1)` is `Tuple(n,
# Int(1))`, and `[n, 1]` at a `List[Int]` slot is `List(n, Int(1),
# __list_literal__=None)`. Struct instances over displays compare by
# application.


struct Tag[t: Tuple[Int, Int]]:
    def __init__(out self):
        pass


struct AT[a: Array[Int, 2]]:
    def __init__(out self):
        pass


comptime T = (5, 1)


def pair[t: Tuple[Int, Int]]():
    print(t[0], t[1])


def labeled[p: Tuple[Int, String]]():
    print(p[0], p[1])


def listed[l: List[Int]]():
    comptime k = len(l)
    comptime z = l[0]
    print(k, z)


def arrayed[a: Array[Int, 2]]():
    comptime s = a[0] + a[1]
    print(s)


def nested[l: List[List[Int]]]():
    comptime k = len(l)
    comptime z = l[1][0]
    print(k, z)


def tupled[t: Tuple[Tuple[Int, Int], Int]]():
    print(t[0][1], t[1])


def defaulted[l: List[Int] = [7, 8, 9]]():
    comptime k = len(l)
    print(k)


def symbolic[n: Int]():
    pair[(n, 1)]()
    listed[[n, 1]]()
    arrayed[[n, 2]]()
    var a: Tag[(n, 1)] = Tag[(n, 1)]()
    var b: Tag[(1 + n, 1)] = Tag[(n + 1, 1)]()
    print("symbolic ok")


def main():
    symbolic[5]()
    pair[(5, 1)]()
    labeled[(1, String("a"))]()
    labeled[(2, "b")]()
    listed[[5, 1]]()
    arrayed[[3, 4]]()
    nested[[[1], [2, 3]]]()
    tupled[((1, 2), 3)]()
    defaulted()
    var c: AT[[5, 1]] = AT[[5, 1]]()
    var d: Tag[T] = Tag[(5, 1)]()
    pair[T]()
    print("closed ok")
