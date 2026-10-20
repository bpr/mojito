# expect: value passed to 'values' cannot be converted from 'KGENParamList[AnyType]'
from std.builtin.variadics import _MLIR


struct L[Tr: type_of(Copyable), //, values: _MLIR.KGENParamListType[Tr]]:
    def __init__(out self):
        pass


def g[*Ts: AnyType]() -> Int:
    _ = L[Ts.values]()
    return 10


def main():
    print(g[Int, String]())
