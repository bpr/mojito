# A KGEN string implements no `__add__`, so two literals' `.value`s do not
# concatenate.
# expect: '__mlir_type.`!kgen.string`' does not implement the '__add__' method
def main():
    comptime both = "a".value + "b".value
