# A type pack's bound holds of each literal element's materialized type:
# `1` meets `Hashable` as the `Int` it binds, as a lone `T: Hashable` does.


def count[*Ts: Hashable](*args: *Ts) -> Int:
    return Ts.__len__()


def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(Ts.__len__()):
        print(args[i])


struct Hasher:
    def __init__(out self):
        pass

    def count[*Ts: Hashable](self, *args: *Ts) -> Int:
        return Ts.__len__()


def main():
    print(count(1))
    print(count(1, "a", 2))
    show(1, "x")
    print(Hasher().count(1, 2))
