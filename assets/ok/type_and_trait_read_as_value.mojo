# A type or a trait read as an expression is a value of its metatype, which
# has no run-time form: a type binder `T: Copyable` reads as a `Copyable`
# value, and a trait, a trait-valued binder, or a `&` composition as its
# `AnyTrait` metatype. A discard of one, or a statement of one, lowers to
# nothing.


def over_type[T: Copyable]():
    _ = T
    T


def over_trait[Tr: type_of(AnyType), T: Tr]():
    _ = Tr
    _ = T


def main():
    over_type[Int]()
    over_trait[Copyable, Int]()
    _ = Copyable
    _ = Copyable & Hashable
    print("ok")
