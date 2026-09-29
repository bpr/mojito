# PROBE (gap): an in-place operator on a module `def`'s reference result.
#
# The pinned Mojo applies `String.__iadd__` through the returned reference
# and prints `ad`. Mojito rejects it: "an in-place operator on the reference
# returned by 'text()'; bind it with 'ref' first". The same operator through
# a method's reference result, or on a `ref` binding of this one, runs.
#
# When fixed: fold into `assets/ok/reference_call_assignment_target.mojo`.
def text(ref t: String) -> ref[origin_of(t)] String:
    return t


def main():
    var s = String("a")
    text(s) += "d"
    print(s)
