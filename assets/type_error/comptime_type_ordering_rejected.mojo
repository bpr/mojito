# expect: 'Movable' does not implement the '__lt__' method
# Types compare only for identity: an ordering of a type parameter calls
# the operator on its metatype, which its first bound does not implement.
def k[T: Movable]():
    comptime if T < Int:
        print("less")


def main():
    k[Int]()
