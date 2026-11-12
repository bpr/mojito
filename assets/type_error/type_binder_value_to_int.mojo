# expect: cannot implicitly convert 'Copyable' value to 'Int'
# A type binder read as a value is its bound's trait type, which converts to
# no storage type.
def g[T: Copyable]():
    var x: Int = T


def main():
    g[Int]()
