# PROBE (subset): an iterator taken from a `List` element is staled by a store
# over that element.
# Upstream (`1.2.0.dev2026092105`) compiles this and prints the replaced
# element's codepoints (`q`, then two spaces).
#
# Mojito rejects with "use of invalidated interior reference 'it' to
# 'ys["element"]~'". A view returned by a method on an element lends the whole
# subtree below the element, because MIR does not see the interior the
# method's return origin names (`view_result_interiors` is checker-only), so
# every store over the element stales every such view. Upstream stales only a
# view whose return origin names an interior below the element
# (`ys[0].rstrip()` names `ys["element"]["bytes"]`), and its diagnostic spells
# that interior where Mojito's spells `~`.
#
# On the fix: decide whether to match upstream here (the iterator reads bytes
# the store destroyed), then promote or move this file accordingly, and delete
# the matching roadmap checkbox.
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var it = ys[0].codepoints()
    ys[0] = String("zz")
    for c in it:
        print(String(c))
