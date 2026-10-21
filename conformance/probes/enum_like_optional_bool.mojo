# PROBE: `Optional` and `Bool` conform to the prelude's `EnumLike`, read through one bound (roadmap R1189).
#
# The pin prints:
#   2 2
#   1 1
#   3
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: the `Optional` half runs; `count[Bool]()` is rejected ("type 'Bool'
# for parameter 'T' does not conform to trait ...EnumLike") until `Bool`
# conforms with its library port (R1189).
# When Mojito runs it, delete it: `assets/ok/enum_like_protocol.mojo` covers
# the `Optional` half.
#
# Run:    mojo run enum_like_optional_bool.mojo
#         cargo run -- run conformance/probes/enum_like_optional_bool.mojo
def count[T: EnumLike]() -> Int:
    return ParameterList[T._enum_case_names]().size


def main():
    print(count[Optional[Int]](), count[Bool]())
    print(Optional(3)._get_enum_discriminant(), True._get_enum_discriminant())
    print(Optional(3)._unsafe_get_enum_payload[1]())
