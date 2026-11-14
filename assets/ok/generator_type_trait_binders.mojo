# A generator type's binder bounded by an enclosing trait-valued binder
# (`From: F`), a generator result naming one (`... To`), a later parameter
# typed by an earlier generator parameter's application (`G: GT[F, T]`,
# `G: GT[Int]`), and an infer-only trait binder solved from the generator
# argument such a parameter binds.

comptime GeneratorTypeGeneratorType[
    From: type_of(AnyType), To: type_of(AnyType)
] = __generator_type[From: From] To

comptime Generator[From: Copyable] = Int

comptime Id[T: AnyType] = T


def bound_by[F: type_of(AnyType), G: __generator_type[From: F] AnyType]():
    print("bound_by")


def result_of[
    F: type_of(AnyType), T: type_of(AnyType), G: __generator_type[From: F] T
]():
    print("result_of")


def applied[
    GT: __generator_type[T: AnyType] AnyType, G: GT[Int]
]():
    print("applied")


def applied_traits[
    F: type_of(AnyType),
    T: type_of(AnyType),
    GT: __generator_type[From: type_of(AnyType), To: type_of(AnyType)] AnyType,
    G: GT[F, T],
]():
    comptime Applied = GT[Copyable, AnyType]
    print("applied_traits")


def inferred[F: type_of(AnyType), //, G: __generator_type[From: F] AnyType]():
    print("inferred")


def inferred_applied[
    F: type_of(AnyType),
    T: type_of(AnyType),
    //,
    GT: __generator_type[From: type_of(AnyType), To: type_of(AnyType)] AnyType,
    G: GT[F, T],
]():
    print("inferred_applied")


def main():
    bound_by[Copyable, Generator]()
    result_of[Copyable, AnyType, Generator]()
    applied[Id, 3]()
    applied_traits[Copyable, AnyType, GeneratorTypeGeneratorType, Generator]()
    inferred[Generator]()
    inferred_applied[GeneratorTypeGeneratorType, Generator]()
