# PROBE (re-probe): a runtime index into a heterogeneous `*args: *Ts` pack.
#
# Both compilers reject from the template. The pin: "invalid call to
# '__getitem_param__': cannot use a dynamic value in a parameter list";
# Mojito: "expected a compile-time Int index, found a runtime value". The
# struct twin is
# `assets/type_error/pack_struct_runtime_index.mojo`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_runtime_index.mojo
#         cargo run -- run conformance/probes/pack_runtime_index.mojo
def f[*Ts: Writable](*a: *Ts):
    var i = 0
    print(a[i])


def main():
    f(1, "two")
