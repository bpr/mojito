# expect: 'Copyable' does not implement the '__eq__' method
# A `comptime if` comparing a type parameter with a value calls the
# operator on the parameter's metatype, which its first bound does not
# implement.
def k[T: Copyable]():
    comptime if T == 1:
        print("one")


def main():
    k[Int]()
