# Capture-list nested `def`s a sibling captures: each declaration builds its
# closure's storage struct, one field per capture, and every call, by name
# or through the sibling, reads the captures from that struct.


struct Noisy(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("del", self.tag)


def copied(n: Int) -> Int:
    var x = n

    def inc() {var x} -> Int:
        return x + 1

    def twice() {inc} -> Int:
        return inc() + inc()

    x = 100
    return twice() + inc() + x


def referenced(k: Int) -> Int:
    var total = k

    def get() {imm total} -> Int:
        return total

    def twice() {get} -> Int:
        return get() + get()

    total = 7
    return twice()


def moved() -> Int:
    var n = Noisy(3)

    def tag() {n^} -> Int:
        return n.tag * 2

    def both() {tag} -> Int:
        return tag() + tag()

    print("before")
    var result = both()
    print("after")
    return result


def mutated() -> Int:
    var box = [40]

    def get() {var box^} -> Int:
        box[0] += 1
        return box[0]

    def twice() {get} -> Int:
        return get() + get()

    return twice() * 100 + get()


def generic_body() -> Int:
    var x = 5

    def scaled[k: Int]() {var x} -> Int:
        return x * k

    def sum() {scaled} -> Int:
        return scaled[2]() + scaled[3]()

    x = 100
    return sum() + scaled[1]() + x


def generic_frame[T: ImplicitlyCopyable & Writable & Deinitable](v: T):
    var x = v

    def show() {var x}:
        print(x)

    def twice() {show}:
        show()
        show()

    twice()


def main():
    print(copied(4))
    print(referenced(1))
    print(moved())
    print(mutated())
    print(generic_body())
    generic_frame(String("hi"))
    generic_frame(3)
