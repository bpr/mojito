# A value parameter holds no storage: each run-time use materializes it
# into a temporary its consumer owns and destroys.


@fieldwise_init
struct Tag(ImplicitlyCopyable):
    var id: Int
    var name: String

    def __deinit__(deinit self):
        print("del", self.id)


def tup[p: Tuple[Int, Tag]]():
    print(p[0])
    print(p[1].name)
    var t = p
    print("kept", t[1].id)


def main():
    tup[(1, Tag(8, "x"))]()
    print("end")
