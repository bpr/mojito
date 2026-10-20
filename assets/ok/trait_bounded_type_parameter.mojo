from std.builtin.variadics import _MLIR


def count[Tr: type_of(AnyType), //, *Ts: Tr]() -> Int:
    return Ts.length


def duplicate[Tr: type_of(Copyable), //, T: Tr](x: T) -> Int:
    var y = x.copy()
    return 1


def need[T: Copyable]() -> Int:
    return 1


def first_copyable[Tr: type_of(Copyable), //, *Ts: Tr]() -> Int:
    return need[Ts[0]]()


def forward[Tr2: type_of(Copyable), //, *Us: Tr2]() -> Int:
    return count[Tr=Tr2, *Us]()


def forward_one[Tr2: type_of(Copyable), //, T: Tr2]() -> Int:
    return need[T]()


def consume[Tr: type_of(AnyType), //, T: Tr](var x: T) -> Int:
    return 5


struct Row[Tr: type_of(AnyType), //, *Ts: Tr]:
    comptime width = Self.Ts.length

    def __init__(out self):
        pass

    def listed(self) -> Int:
        return Listed[Self.Ts.values].n


struct Listed[Tr: type_of(AnyType), //, values: _MLIR.KGENParamListType[Tr]]:
    comptime n = 7

    def __init__(out self):
        pass


struct Spelled[
    Tr: type_of(AnyType), //, values: __mlir_type[`!kgen.param_list<`, Tr, `>`]
]:
    def __init__(out self):
        pass


struct Types[values: __mlir_type[`!kgen.param_list<`, AnyType, `>`]]:
    def __init__(out self):
        pass


def spelled[Tr: type_of(AnyType), //, *Ts: Tr]() -> Int:
    _ = Spelled[Ts.values]()
    return 2


def types[*Ts: AnyType]() -> Int:
    _ = Types[Ts.values]()
    return 3


def solved[*Ts: Copyable]() -> Int:
    var listed: Listed[Tr=Copyable, Ts.values] = Listed[Ts.values]()
    return 8


def widened[*Ts: Copyable]() -> Int:
    _ = Listed[Tr=AnyType, Ts.values]()
    return 16


def forwarded[Tr2: type_of(Copyable), //, *Us: Tr2]() -> Int:
    return Listed[Us.values].n


def kgen_type[T: __mlir_type.`!kgen.type`]() -> Int:
    return 4


def main():
    print(count[Tr=Copyable & Hashable, Int, String]())
    print(Row[Tr=Copyable, Int]().width)
    print(Row[Tr=Copyable, Int, Bool]().listed())
    print(duplicate[Tr=Copyable](3))
    print(first_copyable[Tr=Copyable, Int, String]())
    print(forward[Tr2=Copyable, Int, Bool]())
    print(forward_one[Tr2=Copyable, Int]())
    print(consume[Tr=AnyType](3))
    print(spelled[Tr=Copyable, Int, Bool]())
    print(types[Int, Bool]())
    print(solved[Int, String]())
    print(widened[Int, String]())
    print(forwarded[Tr2=Copyable, Int, Bool]())
    print(kgen_type[Copyable]())
    print(kgen_type[Copyable & Hashable]())
    print(kgen_type[Int]())
