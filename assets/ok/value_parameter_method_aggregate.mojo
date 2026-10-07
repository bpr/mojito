# A method's own tuple or struct value parameter is compile-time data on the
# call: the caller builds nothing, and each instance builds the value it reads.


@fieldwise_init
struct Tag(ImplicitlyCopyable):
    var id: Int
    var name: String

    def __deinit__(deinit self):
        print("del", self.id)


@fieldwise_init
struct Q(ImplicitlyCopyable):
    var a: Int


@fieldwise_init
struct H:
    var k: Int

    def show[p: Tuple[Int, Tag]](self):
        print(self.k, p[0], p[1].name)

    @staticmethod
    def st[p: Tuple[Int, Tag]]():
        print(p[0])

    def pair[p: Tuple[Int, Int]](self):
        print(self.k, p[0], p[1])

    def one[q: Q](self):
        print(self.k, q.a)


def main():
    var h = H(3)
    h.show[(1, Tag(8, "x"))]()
    H.st[(2, Tag(9, "y"))]()
    h.pair[(1, 2)]()
    h.one[Q(4)]()
    print("end")
