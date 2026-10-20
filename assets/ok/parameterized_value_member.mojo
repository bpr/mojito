# A struct's `comptime` value member may take parameters: each application
# binds the member's arguments beside the struct's own, read as a runtime
# value, through `Self` in a method, by keyword, over a symbolic struct
# argument, by a later member, as a struct's parameter argument, and in a
# local constant. An unparameterized member may read the struct's pack.
struct S[*values: Int]:
    comptime get[idx: Int]: Int = Self.values[idx]
    comptime first = Self.values[0]

    def __init__(out self):
        pass

    def second(self) -> Int:
        return Self.get[1]


struct P[n: Int]:
    comptime plus[k: Int] = Self.n + k
    comptime both = Self.plus[1] * 10


@fieldwise_init
struct Arr[n: Int]:
    def size(self) -> Int:
        return Self.n


def shifted[m: Int]() -> Int:
    return P[m].plus[1]


def main():
    print(S[1, 2, 3].get[1])
    print(S[7, 8, 9]().second())
    print(P[10].plus[2])
    print(P[10].plus[k=5])
    print(shifted[5]())
    print(P[2].both)
    var a = Arr[P[3].plus[1]]()
    print(a.size())
    comptime c = P[4].plus[4]
    print(c)
    comptime d = P[4].plus[k=6]
    print(d)
    print(S[4, 5].first)
