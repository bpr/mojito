# A type pack is upstream's `TypeList`: its `length` and `[i]` read the
# bundled struct's members, whether spelled on the pack or on the list.


def count[*Ts: Movable]() -> Int:
    comptime k = Ts.length
    comptime if Ts.length == 2:
        return k + 100
    return k


def runtime_length[*Ts: Movable]() -> Int:
    return Ts.length


def list_length[*Ts: Movable]() -> Int:
    comptime n = TypeList[Ts.values].length
    print(TypeList[Ts.values].length)
    return n + len(TypeList[Ts.values]())


def bounded[Tr: type_of(AnyType), //, *Ts: Tr]() -> Int:
    comptime n = Ts.length
    return n


def first_is_int[*Ts: Movable]() -> Bool:
    comptime if Ts[0] == Int:
        return True
    return False


def show_first[*Ts: Writable](x: TypeList[Ts.values].__getitem_param__[0]):
    print(x)


struct S[*Ts: Writable]:
    comptime n = Self.Ts.length
    comptime m = TypeList[Self.Ts.values].length
    comptime First = Self.Ts[0]

    @staticmethod
    def first_is_int() -> Bool:
        comptime if Self.Ts[0] == Int:
            return True
        return False

    @staticmethod
    def show_first(x: Self.First):
        print(x)


struct B[Tr: type_of(AnyType), //, *Ts: Tr]:
    comptime n = Self.Ts.length


def main():
    print(count[Int, String](), count[Int]())
    print(runtime_length[Int, Int, Int]())
    print(list_length[Int, String]())
    print(bounded[Tr=Copyable, Int, Bool]())
    print(first_is_int[Int, Bool](), first_is_int[Bool, Int]())
    show_first[Int, Bool](7)
    print(S[Int, Bool].n, S[Int, Bool].m)
    print(S[Int, Bool].first_is_int(), S[Bool].first_is_int())
    S[String, Int].show_first("first")
    print(B[Tr=Movable, Int, Bool, Int].n)
