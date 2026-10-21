# expect: 'AnyType' does not implement the '__eq__' method
# A literal pattern over a type subject compares the type's metatype with a
# value, which its bound does not implement.
def k[T: AnyType]():
    comptime match T:
        case 1:
            print("one")
        case _:
            print("other")


def main():
    k[Int]()
