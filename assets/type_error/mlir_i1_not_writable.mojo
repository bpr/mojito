# expect: does not conform to trait 'Writable'
# `__mlir_type.i1` is the MLIR builtin a `Bool` casts to, not a `Bool`: it
# does not conform to `Writable`.


def main():
    print(True.__mlir_i1__())
