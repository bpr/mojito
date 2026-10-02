# PROBE (divergence): a `mut` parameter transferred away and never written
# back.
#
# A `mut` parameter must hold a value when the function returns, so the pin
# rejects a body that moves it out and leaves it empty. Mojito accepts the
# body and prints a reference where the pin would print nothing.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   error: 'x' is uninitialized at return from this function
#   mojito: <ref 1:0>
def take(mut x: String) -> String:
    return x^


def main():
    var s = String("a")
    print(take(s))
