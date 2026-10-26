# PROBE: a `var` binding in a failed case copies when a later case still reads the subject; the subject is evaluated once and survives.
#
# The pin runs it and prints:
#   init 7
#   copy 7
#   del 7
#   guard 7
#   next 7
#   del 7
#   ---
#   init 1
#   copy 1
#   copy 1
#   del 1
#   alt 1
#   del 1
#   ---
#   del 1
#   guard 1
#   done
#   end
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: runs it, but copies the last-use `var` binding of the third match,
# over the named subject `t`, where the pin moves (R1144).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run match_temporary_subject_survives_failed_var_case.mojo
#         cargo run -- run conformance/probes/match_temporary_subject_survives_failed_var_case.mojo
struct Tracked(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v
        print("init", v)

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def guard(v: Int) -> Bool:
    print("guard", v)
    return False


def main():
    match Tracked(7):
        case var x if guard(x.v):
            print("wrong")
        case y:
            print("next", y.v)
    print("---")
    var t = Tracked(1)
    match (t, 0):
        case (var n, 1) | (var n, 0):
            print("alt", n.v)
    print("---")
    match t:
        case var b if guard(b.v):
            print("wrong")
        case _:
            print("done")
    print("end")
