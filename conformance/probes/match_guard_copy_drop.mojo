# PROBE: a `var` binding copies before the guard runs and is destroyed after its last use; a false guard advances to the next case; a temporary subject's last-use binding moves instead of copying.
#
# The pin runs it and prints:
#   init 1
#   match
#   copy 1
#   del 1
#   guard 1
#   copy 1
#   body 1
#   del 5
#   after 1
#   del 1
#   init 7
#   del 7
#   guard 7
#   wildcard
#   end
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: runs it, but copies the last-use `var` binding of the second match where the pin moves (R1144).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run match_guard_copy_drop.mojo
#         cargo run -- run conformance/probes/match_guard_copy_drop.mojo
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
    var t = Tracked(1)
    print("match")
    match t:
        case var x if guard(x.v):
            print("wrong")
        case var y:
            print("body", y.v)
            y.v = 5
    print("after", t.v)
    match Tracked(7):
        case var x if guard(x.v):
            print("wrong")
        case _:
            print("wildcard")
    print("end")
