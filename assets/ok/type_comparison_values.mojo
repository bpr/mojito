# A type comparison is a compile-time `Bool` wherever it is written: a
# `comptime` binding, a runtime argument, a bracket argument, a runtime
# `if`, and a `comptime if` over a struct's type member.
@fieldwise_init
struct Box[E: Movable](Movable):
    var e: Int


struct Row[T: Movable]:
    comptime First = Self.T

    @staticmethod
    def first_is_int():
        comptime if Self.First == Int:
            print("first int")
        else:
            print("first other")


def show[b: Bool]():
    print(b)


def compare[T: Movable, U: Movable]():
    comptime same = T == U
    print(same)
    print(T != Int)
    show[T == Int]()
    if T == List[U]:
        print("list of U")
    comptime if T == Box[Int]:
        print("box of Int")


def element[*Ts: Movable]():
    comptime F = Ts[0]
    comptime b = F == Int
    print(b)
    print(Ts[1] == Bool)


def main():
    compare[Int, Int]()
    compare[List[Bool], Bool]()
    compare[Box[Int], Int]()
    Row[Int].first_is_int()
    Row[Bool].first_is_int()
    element[Int, Bool]()
    element[Bool, Int]()
    print(Int != Bool)
