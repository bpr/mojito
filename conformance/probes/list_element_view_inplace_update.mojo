# PROBE (subset): an in-place update of a `List` element under a live view of
# its bytes is not rejected.
# Upstream (`1.2.0.dev2026092105`) rejects the `print` with "use of
# invalidated interior reference 'ys["element"]["bytes"]'".
#
# Mojito accepts the program and the VM traps with "use after Pointer
# deallocation": the augmented store through the element reference records no
# interior invalidation, where `ys[0] = ...` and `ys.append(...)` do.
#
# On the fix: move this file to `assets/ownership_error/` with its ledger row
# and delete the matching roadmap checkbox.
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    ys[0] += "tail"
    print(v)
