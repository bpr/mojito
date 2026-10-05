# A method whose `comptime if` or `comptime for` reads only its own binders
# is served by its template, as a generic `def` is: the elaborator decides
# each branch and unrolls each loop per call, whether the binder is spelled
# or inferred, on a plain struct or a generic struct's instance.
struct S:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def label[T: AnyType](self) -> String:
        comptime if T == Int:
            return "int"
        else:
            return "other"

    def kind[T: Copyable](self, x: T) -> Int:
        comptime if T == Int:
            return self.n
        else:
            return -self.n

    def sum_to[k: Int](self) -> Int:
        var total = 0
        comptime for i in range(k):
            total += i * self.n
        return total


@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var item: Self.T

    def tag[U: AnyType](self) -> String:
        comptime if U == Bool:
            return "bool"
        else:
            return "not bool"


def main():
    var s = S(2)
    print(s.label[Int](), s.label[String]())
    print(s.kind(7), s.kind(1.5))
    print(s.sum_to[4](), s.sum_to[1]())
    var b = Box[Int](3)
    print(b.tag[Bool](), b.tag[Int]())
