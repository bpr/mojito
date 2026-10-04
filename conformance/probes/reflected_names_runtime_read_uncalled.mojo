# PROBE: a runtime read of a `field_names()` binding in a generic body that is
# never called.
#
# Mojito runs it and prints nothing; the pin rejects `print(names[i])` in the
# template itself ("cannot materialize comptime value of type
# 'Array[StringSpan[ImmStaticOrigin], ...]' to runtime because it is not
# 'ImplicitlyCopyable'"). Calling `show[Point]()` makes Mojito reject it too
# (assets/type_error/comptime_field_names_runtime_use.mojo). Mojito accepts a
# program the pin rejects, so this is a divergence.
#
# Observed 2026-10-04 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run reflected_names_runtime_read_uncalled.mojo
#         cargo run -- run conformance/probes/reflected_names_runtime_read_uncalled.mojo
def show[T: AnyType]():
    comptime names = reflect[T].field_names()
    comptime for i in range(len(names)):
        print(names[i])


def main():
    pass
