# A struct's `comptime` member may name the struct itself, as `Self` or as
# the struct applied to a computed parameter, and a type member read through
# the struct is the type it names: a static call, a parameter read, a
# construction, and a call through `Self.<member>` inside the struct. The
# pinned Mojo prints `4`, `5`, `10`, `5`, `5`, `7`, and `15`.
struct T[n: Int]:
    comptime current = Self
    comptime other = T[Self.n + 1]
    comptime boxed = Box[Self.n + 1]

    @staticmethod
    def answer() -> Int:
        return Self.n

    @staticmethod
    def next_answer() -> Int:
        return Self.other.answer()


struct U[n: Int]:
    @staticmethod
    def answer() -> Int:
        return Self.n


@fieldwise_init
struct Box[n: Int](TrivialRegisterPassable):
    var x: Int

    def get(self) -> Int:
        return Self.n + self.x


struct Plain:
    comptime target = U[7]


def main():
    print(T[4].current.answer())
    print(T[4].other.answer())
    print(T[4].other.other.current.n + T[4].n)
    print(T[4].next_answer())
    comptime X = T[4].other
    print(X.answer())
    print(Plain.target.answer())
    print(T[4].boxed(10).get())
