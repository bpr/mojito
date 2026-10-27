# expect: 'Movable' does not implement the '__le__' method
# A type comparison bound by a `comptime` value orders nothing either.
def k[T: Movable]():
    comptime b = T <= Int
    print(b)


def main():
    k[Int]()
