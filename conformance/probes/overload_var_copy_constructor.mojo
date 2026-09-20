# PROBE (divergence): a constructor overload set drops its generic candidates.
#
# The place `s` costs `__init__(out self, var a: String)` the copy it makes,
# so the pin selects the generic constructor; an rvalue costs nothing and both
# select the `var` one. Mojito ranks neither call: `decls_are_concrete`
# (`checker/declarations.rs`) retains only concrete candidates whenever one
# matches, at both constructor selection sites, so the generic constructor is
# discarded before scoring. The same pair declared as free functions or as
# methods now matches the pin (`assets/ok/overload_var_copy_beside_generic.mojo`,
# `assets/ok/overload_var_copy_method.mojo`).
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    2 then 1
#   mojito: 1 then 1
#
# Run:    mojo run overload_var_copy_constructor.mojo
#         cargo run -- run conformance/probes/overload_var_copy_constructor.mojo
#
# When fixed: the filter must tell a per-call clone of a generic constructor
# from a separately declared concrete overload; this file moves to `assets/ok/`
# beside the two above.
struct C:
    var v: Int

    def __init__(out self, var a: String):
        self.v = 1

    def __init__[T: Writable](out self, a: T):
        self.v = 2


def main():
    var s = String("s")
    print(C(s).v)
    print(C(String("t")).v)
