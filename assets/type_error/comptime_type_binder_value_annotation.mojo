# A type binder's value is its metatype, the trait it is bounded by, which
# converts to no value type: a `comptime` name annotated `Int` cannot bind it.
# expect: cannot implicitly convert 'Writable & Copyable' value to 'Int'
def go[T: Copyable & Writable]():
    comptime x: Int = T
    pass


def main():
    go[Int]()
