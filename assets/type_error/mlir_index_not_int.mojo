# The builtin `index` `__mlir_index__` returns is not an `Int`: only
# `Int(mlir_value=)` converts it, as at the pin.
# expect: type mismatch for variable 'i': expected Int, found __mlir_type.index
def main():
    var i: Int = Int(3).__mlir_index__()
    print(i)
