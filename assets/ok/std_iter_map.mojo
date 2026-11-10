# `map[function](iterable)`: upstream's lazy `_MapIterator`, borrowing an
# lvalue source and consuming a temporary one, over a function and a static
# method.
struct Scale:
    @staticmethod
    def triple(var x: Int) -> Int:
        return x * 3


def inc(var x: Int) -> Int:
    return x + 1


def main() raises:
    var l = [1, 2, 3]
    for e in map[inc](l):
        print(e)
    var m = map[Scale.triple](l)
    print(m.bounds()[0], next(m), next(m))
    for e in map[inc]([10, 20]):
        print(e)
    print(len(l))
