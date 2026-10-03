# Probe: a compile-time evaluation whose callee needs three instances of a
# generic `def`.
#
# The pin and Mojito (2026-10-02) both print `6`. On the request path of
# docs/notes/ctfe-request-path.md the three instances are demanded from the
# elaborator's worklist as the reference closure of `k`.
def twice[T: Copyable](x: T) -> Int:
    return 2

def k() -> Int:
    return twice(1) + twice("s") + twice(2.5)

comptime K = k()

def main():
    print(K)
