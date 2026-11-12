# expect: 'AnyTrait[AnyType]' does not implement the '__eq__' method
# A trait read as a value is its `AnyTrait` metatype, which implements no
# comparison.
def f[Tr: type_of(AnyType)]():
    _ = Tr == Copyable


def main():
    f[Copyable]()
