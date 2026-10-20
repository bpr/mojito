# PROBE: `Optional` and `Bool` conform to the prelude's `EnumLike`, read through one bound (roadmap R1132).
#
# The pin prints:
#   2 2
#   1 1
#   3
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects the bound ("unknown trait 'EnumLike' in a type-parameter bound").
# When Mojito runs it, delete it: R1132's `assets/ok` fixture covers it.
#
# Run:    mojo run enum_like_optional_bool.mojo
#         cargo run -- run conformance/probes/enum_like_optional_bool.mojo
def count[T: EnumLike]() -> Int:
    return ParameterList[T._enum_case_names]().size


def main():
    print(count[Optional[Int]](), count[Bool]())
    print(Optional(3)._get_enum_discriminant(), True._get_enum_discriminant())
    print(Optional(3)._unsafe_get_enum_payload[1]())
