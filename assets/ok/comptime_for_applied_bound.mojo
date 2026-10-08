# A `comptime for` range bound, or a binding in its body, that applies a
# function whatever its result type (`range(len(mk(n)))`, `comptime e =
# mk(n)[i]`, `comptime w = label(i)`) is evaluated per instance from the
# template: no call clones the body.


def mk(n: Int) -> List[Int]:
    var values = List[Int]()
    for i in range(n):
        values.append(i * 10)
    return values^


def label(n: Int) -> String:
    return String("L") + String(n)


def self_offset() -> Int:
    return 100


def walk[n: Int]():
    comptime for i in range(len(mk(n))):
        comptime e = mk(n)[i]
        comptime w = label(i)
        print(i, e, w)


struct Walker:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def walk[n: Int](self):
        comptime for i in range(len(mk(n)) + 1):
            comptime w = label(i + self_offset())
            print(w, self.v)


def main():
    walk[2]()
    walk[3]()
    Walker(5).walk[1]()
    Walker(6).walk[2]()
