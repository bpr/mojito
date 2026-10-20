# A KGEN string implements no `__eq__`, so two literals' `.value`s do not
# compare.
# expect: operator '==' is not defined for __mlir_type.`!kgen.string` and __mlir_type.`!kgen.string`
def main():
    comptime if "a".value == "a".value:
        print("equal")
