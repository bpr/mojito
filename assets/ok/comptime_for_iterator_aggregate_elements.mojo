# `comptime for` over user iterators of `String` and struct elements: each
# copy materializes its own element, which a nested `def` may read.
@fieldwise_init
struct Words(Iterator, Copyable):
    comptime Element = String
    var n: Int

    def __next__(mut self) raises StopIteration -> String:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return String("w") + String(self.n)

    def __iter__(self) -> Self:
        return self.copy()


@fieldwise_init
struct Point(Copyable, Writable):
    var a: Int
    var b: Int


@fieldwise_init
struct Points(Iterator, Copyable):
    comptime Element = Point
    var n: Int

    def __next__(mut self) raises StopIteration -> Point:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return Point(self.n, self.n * 10)

    def __iter__(self) -> Self:
        return self.copy()


def main():
    comptime for w in Words(2):
        print(w)
    comptime for p in Points(2):
        print(p.a, p.b)
    comptime for p in Points(3):

        def show():
            print("n", p.a)

        show()
