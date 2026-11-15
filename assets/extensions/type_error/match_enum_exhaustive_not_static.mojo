# A `SumType` whose `_enum_is_exhaustive` is a parameter cannot be
# matched in a body where the parameter is unknown.
# `SumType` and `EnumLike` are upstream's enum proposal's traits, which the
# pin does not declare (`docs/non-goals.md`).
# expect: '_enum_is_exhaustive' must be statically known when matching a SumType


struct E[flag: Bool](EnumLike):
    var tag: Int

    comptime _sum_case_names = ParameterList.of["A".value, "B".value].values
    comptime _enum_is_exhaustive = Self.flag

    def __init__(out self, tag: Int):
        self.tag = tag

    def _get_sum_discriminant(self) -> Int:
        return self.tag


def f[b: Bool](e: E[b]):
    match e:
        case .A:
            print("a")
        case _:
            print("other")


def main():
    f(E[True](0))
