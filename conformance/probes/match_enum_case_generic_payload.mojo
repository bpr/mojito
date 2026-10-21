# PROBE: the payload type of an `Optional[T]` enum case (a `TypeList` element of the symbolic application's `_enum_case_types`) is a fresh `T` without the caller's bounds.
#
# The pin runs it and prints:
#   some 1
#   none
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects `print(x)` ("an element of 'values' with type 'T' does not
# conform to trait 'Writable'") until R1236 lands. Annotating a local with
# the element (`var z: Optional[T]._enum_elt_type_for_case[1]`) fails as
# "expected T, found T" in Mojito; the pin rejects that form too, later
# ("'z' abandoned without being explicitly destroyed").
#
# Run:    mojo run match_enum_case_generic_payload.mojo
#         cargo run -- run conformance/probes/match_enum_case_generic_payload.mojo
def h[T: Copyable & Writable](o: Optional[T]):
    match o:
        case .Some(x):
            print("some", x)
        case .None:
            print("none")


def main():
    h(Optional(1))
    h(Optional[String]())
