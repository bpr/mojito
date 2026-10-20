# expect: value passed to 'values' cannot be converted from 'KGENParamList[Int]'
from std.builtin.variadics import _MLIR


struct L[Tr: type_of(AnyType), //, values: _MLIR.KGENParamListType[Tr]]:
    def __init__(out self):
        pass


def g[*vs: Int]() -> Int:
    _ = L[vs.values]()
    return 11


def main():
    print(g[1, 2]())
