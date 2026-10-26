# PROBE: a capturing lambda's closure value passed to a callable binder and
# called twice.
#
# The pin runs it and prints:
#   move 3
#   33
#   copy 4
#   copy 4
#   44
#   copy 4
#   5
#
# Mojito prints `copy 4` once on the value line, on both backends: each
# value is the lambda's storage struct, which `twice` borrows, so no call
# copies it. The pin's two copies are both made where the storage is
# built (R1432); the immediately invoked lambda's single copy agrees.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-25 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run closure_value_copies_its_storage.mojo
#         cargo run -- run conformance/probes/closure_value_copies_its_storage.mojo
struct Tracked(Movable, ImplicitlyCopyable):
    var n: Int
    def __init__(out self, n: Int):
        self.n = n
    def __init__(out self, *, deinit move: Self):
        print("move", move.n)
        self.n = move.n
    def __init__(out self, *, copy: Self):
        print("copy", copy.n)
        self.n = copy.n

def twice[F: def() -> Int](f: F) -> Int:
    return f() * 10 + f()

def main():
    var t = Tracked(3)
    print(twice(lambda {var t^} -> Int: t.n))
    var u = Tracked(4)
    print(twice(lambda {var u} -> Int: u.n))
    print((lambda {var u} -> Int: u.n + 1)())
