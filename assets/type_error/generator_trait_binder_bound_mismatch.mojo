# A generator type's binder bounded by an earlier trait-valued binder is
# bounded by the trait that binder receives, so a generator whose binder is
# bounded by another trait does not convert.
# expect: invalid call to 'bound_by': 'bound_by' parameter 'G' has '__generator_type[From: Hashable] AnyType' type, but value has type '__generator_type[From: Copyable] AnyType'
comptime Generator[From: Copyable] = Int


def bound_by[F: type_of(AnyType), G: __generator_type[From: F] AnyType]():
    print("bound_by")


def main():
    bound_by[Hashable, Generator]()
