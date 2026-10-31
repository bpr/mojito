# A static method read through its struct is a function value: stored,
# passed, specialized by a bracket, selected among overloads by the
# expected type, and read through `Self` inside the struct.
struct S:
    @staticmethod
    def st(x: Int) -> Int:
        return x + 1

    @staticmethod
    def gst[k: Int](x: Int) -> Int:
        return x + k

    @staticmethod
    def twice(x: Int) -> Int:
        return x * 2

    @staticmethod
    def twice(x: String) -> String:
        return x + x

    @staticmethod
    def through_self(x: Int) -> Int:
        var f = Self.st
        return f(x)


def apply(f: def(Int) thin -> Int, x: Int) -> Int:
    return f(x)


def main():
    var f = S.st
    print(f(3))
    var g = S.gst[10]
    print(g(3))
    var k = S.gst[k=20]
    print(k(1))
    print(apply(S.st, 5))
    print(apply(S.gst[100], 5))
    print(S.through_self(7))
    var h: def(Int) thin -> Int = S.twice
    print(h(21))
