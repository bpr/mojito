# PROBE (native): a fresh struct value passed to `Tuple`'s initializer.
#
# The pinned Mojo and Mojito's VM move the `Q` temporary into the tuple.
# Mojito's native backend copies it in (running the printing copy
# initializer) and never destroys the temporary.
#
# Observed 2026-10-07 against `Mojo 1.2.0.dev2026092105`:
#   mojo:   1 / del 8 / end
#   mojito: VM matches; `--backend pliron` prints copy 8 / 1 / del 8 / end
#
# When fixed: give the struct in
# `assets/ok/value_parameter_materialized_per_use.mojo` this printing copy
# initializer.


@fieldwise_init
struct Q(ImplicitlyCopyable):
    var a: Int
    var s: String

    def __init__(out self, *, copy: Self):
        self.a = copy.a
        self.s = copy.s
        print("copy", self.a)

    def __deinit__(deinit self):
        print("del", self.a)


def main():
    var t = (1, Q(8, "x"))
    print(t[0])
    print("end")
