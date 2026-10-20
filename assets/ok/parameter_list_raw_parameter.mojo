# A user's parameter typed by upstream's raw list type
# (`raw: _MLIR.KGENParamListType[t]`) binds a pack's `vs.values` and
# applies `ParameterList` to it; its element type is inferred from the list.
from std.builtin.variadics import _MLIR


def h[t: AnyType, //, raw: _MLIR.KGENParamListType[t]]():
    print(ParameterList[raw].size)


def show[t: Copyable & Writable, //, raw: _MLIR.KGENParamListType[t]]():
    for v in ParameterList[raw]():
        print(v)


def f[*vs: Int]():
    h[vs.values]()
    show[vs.values]()


def main():
    f[7, 8]()
    f[1, 2, 3]()
