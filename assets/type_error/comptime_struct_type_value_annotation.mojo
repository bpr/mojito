# A struct type bound where its annotation declares a value converts through
# its metatype, `AnyStruct[Int]`, which converts to no value type.
# expect: cannot implicitly convert 'AnyStruct[Int]' value to 'String'
def go[T: Copyable]():
    comptime x: String = Int
    pass


def main():
    go[Int]()
