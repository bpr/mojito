# PROBE: bare, `var`, `ref`, and `as` bindings over a temporary subject each see one value and no copy.
#
# The pin runs it and prints:
#   make
#   as 3
#   del 3
#   ---
#   make
#   bare 3
#   del 3
#   ---
#   make
#   var 3
#   del 3
#   ---
#   make
#   ref 3
#   del 3
#   end
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: runs it, but copies the `var` binding of a dead temporary where the pin moves (R1144).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run match_binding_kinds_on_temporary.mojo
#         cargo run -- run conformance/probes/match_binding_kinds_on_temporary.mojo
struct Box(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def make() -> Box:
    print("make")
    return Box(3)


def main():
    match make():
        case _ as whole:
            print("as", whole.v)
    print("---")
    match make():
        case x:
            print("bare", x.v)
    print("---")
    match make():
        case var x:
            print("var", x.v)
    print("---")
    match make():
        case ref x:
            print("ref", x.v)
    print("end")
