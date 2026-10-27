# A KGEN string implements no `__eq__`, so two literals' `.value`s do not
# compare.
# expect: '__mlir_type.`!kgen.string`' does not implement the '__eq__' method
def main():
    comptime if "a".value == "a".value:
        print("equal")
