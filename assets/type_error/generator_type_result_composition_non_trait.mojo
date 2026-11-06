# A generator type's `&`-composed result composes traits; a non-trait member
# is the composition's missing `__and__`.
# expect: 'AnyTrait[Copyable]' does not implement the '__and__' method
def both[G: __generator_type[T: Copyable] Copyable & Int]() -> Int:
    return 1


def main():
    print(1)
