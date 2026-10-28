# An integer literal is not an `index`: a parameter of type
# `__mlir_type.index` takes only an `index` value, as at the pin.
# expect: type mismatch for value parameter 'n': expected __mlir_type.index, found Int
def f[n: __mlir_type.index]():
    pass


def main():
    f[3]()
