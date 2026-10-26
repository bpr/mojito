# PROBE: a display of one capturing lambda, called through its element.
#
# The pin runs it and prints:
#   5 5
#   3 4
#
# Mojito fails to verify after `automatic-inline`: the caller's slot is
# declared `Array[def() capturing[] -> Int, 1]`, and the inlined erased
# instance types it `Array[def() -> Int, _REMOVED_ARG]` (R1429). With
# `AUTOMATIC_INLINE` removed from `post_elaboration.rs` it prints the pin's
# output on both backends.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-25 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run capturing_lambda_display_inlined.mojo
#         cargo run -- run conformance/probes/capturing_lambda_display_inlined.mojo
struct Counter(ImplicitlyCopyable, Movable, TrivialRegisterPassable):
    var n: Int
    def __init__(out self, n: Int):
        self.n = n
    def bump(mut self) -> Int:
        self.n += 1
        return self.n

def main():
    var c = Counter(4)
    var fs = [lambda {var c^} -> Int: c.bump()]
    print(fs[0](), fs[0]())
    var s = String("ab")
    var gs = [lambda {var s^} -> Int: (s.__iadd__("!"), s.byte_length())[1]]
    print(gs[0](), gs[0]())
