# A static method called on a bare struct name binds the struct's
# defaulted parameters: `T` in `T.answer()` is `T[3]`, as `T[]` is.


struct T[n: Int = 3]:
    @staticmethod
    def answer() -> Int:
        return Self.n

    @staticmethod
    def scaled[k: Int]() -> Int:
        return Self.n * k

    @staticmethod
    def plus(x: Int) -> Int:
        return Self.n + x


struct Inferred[n: Int = 7, //]:
    @staticmethod
    def answer() -> Int:
        return Self.n


struct Mixed[E: Copyable & Writable, n: Int = 2]:
    @staticmethod
    def show(x: Self.E) -> String:
        return String(x) * Self.n


def generic[m: Int]() -> Int:
    return T.answer() + T[m].answer()


def main():
    print(T.answer())
    print(T[].answer())
    print(T[5].answer())
    print(T.scaled[2]())
    print(T[5].scaled[2]())
    print(T.plus(4))
    print(Inferred.answer())
    print(Mixed.show("ab"))
    print(generic[10]())
